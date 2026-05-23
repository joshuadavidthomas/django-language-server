use std::collections::BTreeMap;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::sync::mpsc;

use camino::Utf8PathBuf;
use serde::Deserialize;
use serde_json::json;
use tempfile::NamedTempFile;
use wait_timeout::ChildExt;

use crate::enrichment::ProjectEnrichment;
use crate::enrichment::ProjectEnrichmentIssue;
use crate::enrichment::RuntimeUnavailableKind;
use crate::names::LibraryName;
use crate::names::PyModuleName;
use crate::project::Project;
use crate::root_discovery::ProjectRootDiscovery;
use crate::Db;
use crate::DjangoEnvironmentCandidatesOutcome;
use crate::Interpreter;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeEnrichmentRequest {
    pub(crate) python: Utf8PathBuf,
    pub(crate) project_root: Utf8PathBuf,
    pub(crate) django_settings_module: Option<String>,
    pub(crate) pythonpath: Vec<Utf8PathBuf>,
    pub(crate) env_vars: Vec<(String, String)>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct InspectorEnrichment {
    #[serde(default)]
    template_libraries: BTreeMap<String, String>,
}

impl From<InspectorEnrichment> for ProjectEnrichment {
    fn from(enrichment: InspectorEnrichment) -> Self {
        Self::Fresh(
            enrichment
                .template_libraries
                .into_iter()
                .filter_map(|(name, module)| {
                    Some((
                        LibraryName::parse(&name).ok()?,
                        PyModuleName::parse(&module).ok()?,
                    ))
                })
                .collect(),
        )
    }
}

#[tracing::instrument(
    level = "info",
    skip_all,
    fields(
        outcome,
        project_root,
        python,
        django_settings_module,
        pythonpath_entries,
        env_var_count,
        template_library_count,
        status,
    )
)]
pub fn load_runtime_project_enrichment(db: &dyn Db, project: Project) -> ProjectEnrichment {
    let request = match runtime_enrichment_request(db, project) {
        Ok(request) => request,
        Err(issue) => return ProjectEnrichment::Unresolved(issue),
    };
    let span = tracing::Span::current();
    span.record("project_root", request.project_root.as_str());
    span.record("python", request.python.as_str());
    span.record(
        "django_settings_module",
        tracing::field::debug(&request.django_settings_module),
    );
    span.record("pythonpath_entries", request.pythonpath.len());
    span.record("env_var_count", request.env_vars.len());

    let result = InspectorCommand::for_request(&request).and_then(InspectorCommand::run);

    match result {
        Ok(enrichment) => {
            tracing::Span::current().record("outcome", "fresh");
            enrichment.into()
        }
        Err(kind) => {
            tracing::Span::current().record("outcome", "failed");
            tracing::warn!(failure = ?kind, "Runtime enrichment provider failed");
            crate::ProjectEnrichment::Unresolved(crate::ProjectEnrichmentIssue::InspectorFailed(
                kind,
            ))
        }
    }
}

#[tracing::instrument(level = "info", skip_all, fields(outcome))]
fn runtime_enrichment_request(
    db: &dyn Db,
    project: Project,
) -> Result<RuntimeEnrichmentRequest, ProjectEnrichmentIssue> {
    let discovery = project.root_discovery(db);
    let ProjectRootDiscovery::Ready(discovery) = discovery else {
        tracing::Span::current().record("outcome", "environment_not_configured");
        return Err(ProjectEnrichmentIssue::RuntimeUnavailable {
            interpreter: None,
            kind: RuntimeUnavailableKind::EnvironmentNotConfigured,
        });
    };
    let DjangoEnvironmentCandidatesOutcome::Ready { candidates, .. } =
        crate::django_environment_candidates(db, project)
    else {
        tracing::Span::current().record("outcome", "environment_not_configured");
        return Err(ProjectEnrichmentIssue::RuntimeUnavailable {
            interpreter: None,
            kind: RuntimeUnavailableKind::EnvironmentNotConfigured,
        });
    };
    let Some(candidate) = candidates.first() else {
        tracing::Span::current().record("outcome", "environment_not_configured");
        return Err(ProjectEnrichmentIssue::RuntimeUnavailable {
            interpreter: None,
            kind: RuntimeUnavailableKind::EnvironmentNotConfigured,
        });
    };
    let root = candidate
        .root()
        .and_then(|path| discovery.roots().iter().find(|root| root.root(db) == path))
        .or_else(|| discovery.roots().first())
        .expect("ready discovery has at least one root");
    let project_root = root.root(db).clone();
    let interpreter = root.interpreter(db).clone().unwrap_or(Interpreter::Auto);
    let Some(python) = interpreter.python_path(&project_root) else {
        tracing::Span::current().record("outcome", "missing_python");
        return Err(ProjectEnrichmentIssue::RuntimeUnavailable {
            interpreter: Some(interpreter),
            kind: RuntimeUnavailableKind::MissingPython,
        });
    };

    let request = RuntimeEnrichmentRequest {
        python,
        project_root,
        django_settings_module: Some(candidate.settings().as_str().to_string()),
        pythonpath: root.pythonpath(db).clone(),
        env_vars: root.env_vars(db).entries().to_vec(),
    };
    tracing::Span::current().record("outcome", "ready");
    Ok(request)
}

struct InspectorZipapp(NamedTempFile);

impl InspectorZipapp {
    const BYTES: &'static [u8] = include_bytes!(concat!(env!("OUT_DIR"), "/djls_inspector.pyz"));

    fn create() -> Result<Self, crate::InspectorFailureKind> {
        let mut file = NamedTempFile::with_prefix("djls_inspector_")
            .map_err(|_| crate::InspectorFailureKind::SubprocessFailed { status: None })?;
        file.write_all(Self::BYTES)
            .map_err(|_| crate::InspectorFailureKind::SubprocessFailed { status: None })?;
        file.flush()
            .map_err(|_| crate::InspectorFailureKind::SubprocessFailed { status: None })?;
        Ok(Self(file))
    }

    fn path(&self) -> &std::path::Path {
        self.0.path()
    }
}

struct InspectorCommand {
    command: Command,
    zipapp: InspectorZipapp,
}

impl InspectorCommand {
    fn for_request(
        request: &RuntimeEnrichmentRequest,
    ) -> Result<Self, crate::InspectorFailureKind> {
        let zipapp = InspectorZipapp::create()?;
        let mut command = Command::new(request.python.as_std_path());
        command
            .arg(zipapp.path())
            .current_dir(request.project_root.as_std_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        command.process_group(0);
        if let Some(settings) = &request.django_settings_module {
            command.env("DJANGO_SETTINGS_MODULE", settings);
        }
        if !request.pythonpath.is_empty() {
            let joined = std::env::join_paths(
                request
                    .pythonpath
                    .iter()
                    .map(|path| path.as_path().as_std_path()),
            )
            .map_err(|_| crate::InspectorFailureKind::SubprocessFailed { status: None })?;
            command.env("PYTHONPATH", joined);
        }
        for (key, value) in &request.env_vars {
            command.env(key, value);
        }
        Ok(Self { command, zipapp })
    }

    fn run(self) -> Result<InspectorEnrichment, crate::InspectorFailureKind> {
        self.spawn()?.run()
    }

    fn spawn(mut self) -> Result<InspectorProcess, crate::InspectorFailureKind> {
        let child = self
            .command
            .spawn()
            .map_err(|_| crate::InspectorFailureKind::SubprocessFailed { status: None })?;
        Ok(InspectorProcess {
            child,
            waited: false,
            _zipapp: self.zipapp,
        })
    }
}

struct InspectorProcess {
    child: Child,
    waited: bool,
    _zipapp: InspectorZipapp,
}

impl InspectorProcess {
    const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    fn run(&mut self) -> Result<InspectorEnrichment, crate::InspectorFailureKind> {
        self.write_queries()?;
        let enrichment = self.read_enrichment_async()?;
        self.wait()?;
        match enrichment.recv_timeout(Self::TIMEOUT) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.kill_process_group();
                Err(crate::InspectorFailureKind::TimedOut)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(crate::InspectorFailureKind::SubprocessFailed { status: None })
            }
        }
    }

    fn write_queries(&mut self) -> Result<(), crate::InspectorFailureKind> {
        let result = (|| {
            let stdin = self.child.stdin.as_mut()?;
            writeln!(stdin, "{}", json!({ "query": "template_libraries" })).ok()?;
            Some(())
        })();
        result.ok_or(crate::InspectorFailureKind::SubprocessFailed { status: None })?;
        drop(self.child.stdin.take());
        Ok(())
    }

    fn read_enrichment_async(
        &mut self,
    ) -> Result<
        mpsc::Receiver<Result<InspectorEnrichment, crate::InspectorFailureKind>>,
        crate::InspectorFailureKind,
    > {
        let stdout = self
            .child
            .stdout
            .take()
            .ok_or(crate::InspectorFailureKind::SubprocessFailed { status: None })?;
        let span = tracing::Span::current();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _enter = span.enter();
            let mut lines = BufReader::new(stdout).lines();
            let result = (|| {
                let template_libraries =
                    InspectorResponse::<InspectorTemplateLibraries>::parse_line(lines.next())?;

                tracing::Span::current()
                    .record("template_library_count", template_libraries.libraries.len());

                Ok(InspectorEnrichment {
                    template_libraries: template_libraries.libraries,
                })
            })();
            let _ = sender.send(result);
        });
        Ok(receiver)
    }

    fn wait(&mut self) -> Result<(), crate::InspectorFailureKind> {
        let Some(status) = self
            .child
            .wait_timeout(Self::TIMEOUT)
            .map_err(|_| crate::InspectorFailureKind::SubprocessFailed { status: None })?
        else {
            self.kill_process_group();
            let _ = self.child.wait();
            self.waited = true;
            return Err(crate::InspectorFailureKind::TimedOut);
        };
        self.waited = true;
        tracing::Span::current().record("status", tracing::field::debug(status.code()));
        if status.success() {
            Ok(())
        } else {
            Err(crate::InspectorFailureKind::SubprocessFailed {
                status: status.code(),
            })
        }
    }

    fn kill_process_group(&mut self) {
        #[cfg(unix)]
        {
            if let Ok(process_group) = i32::try_from(self.child.id()) {
                // SAFETY: the inspector is spawned into its own process group.
                unsafe {
                    let _ = libc::kill(-process_group, libc::SIGKILL);
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = self.child.kill();
        }
    }
}

impl Drop for InspectorProcess {
    fn drop(&mut self) {
        if self.waited {
            return;
        }
        if let Ok(None) = self.child.try_wait() {
            self.kill_process_group();
            let _ = self.child.wait();
        }
    }
}

#[derive(Deserialize)]
struct InspectorResponse<T> {
    ok: bool,
    data: Option<T>,
}

impl<T: for<'de> Deserialize<'de>> InspectorResponse<T> {
    fn parse_line(line: Option<std::io::Result<String>>) -> Result<T, crate::InspectorFailureKind> {
        let Some(line) = line else {
            return Err(crate::InspectorFailureKind::InvalidJson);
        };
        let Ok(line) = line else {
            return Err(crate::InspectorFailureKind::SubprocessFailed { status: None });
        };
        let Ok(response) = serde_json::from_str::<Self>(&line) else {
            return Err(crate::InspectorFailureKind::InvalidJson);
        };
        if !response.ok {
            return Err(crate::InspectorFailureKind::SubprocessFailed { status: None });
        }
        response
            .data
            .ok_or(crate::InspectorFailureKind::InvalidJson)
    }
}

#[derive(Deserialize)]
struct InspectorTemplateLibraries {
    libraries: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use djls_source::SourceFiles;
    use salsa::Setter;

    use super::*;
    use crate::enrichment::ProjectEnrichment;
    use crate::root_discovery::DjangoEnvironmentSeed;
    use crate::root_discovery::DjangoSettingsModuleSeed;
    use crate::root_discovery::ProjectEnvVars;
    use crate::root_discovery::ProjectRootDiscovery;
    use crate::root_discovery::ProjectRootDiscoverySet;
    use crate::root_discovery::RootDiscoveryInput;
    use crate::source_files::SourceFileInventory;
    use crate::source_files::SourceFilesIssue;

    #[salsa::db]
    #[derive(Default)]
    struct TestDb {
        storage: salsa::Storage<Self>,
        files: SourceFiles,
        project: OnceLock<Project>,
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    #[salsa::db]
    impl djls_source::Db for TestDb {
        fn files(&self) -> &SourceFiles {
            &self.files
        }

        fn read_file(&self, _path: &camino::Utf8Path) -> std::io::Result<String> {
            Ok(String::new())
        }
    }

    #[salsa::db]
    impl crate::Db for TestDb {
        fn project(&self) -> Project {
            *self.project.get().expect("test project initialized")
        }
    }

    impl TestDb {
        fn with_project() -> Self {
            let db = Self::default();
            db.project
                .set(Project::new(
                    &db,
                    SourceFileInventory::Unavailable {
                        issue: SourceFilesIssue::NotLoaded,
                    },
                    ProjectRootDiscovery::Absent,
                    ProjectEnrichment::Absent,
                ))
                .expect("project should initialize once");
            db
        }
    }

    fn executable_python(root: &camino::Utf8Path) -> Utf8PathBuf {
        let python = root.join("python");
        std::fs::write(&python, "").expect("python placeholder should be writable");
        python
    }

    #[test]
    fn runtime_enrichment_request_uses_matching_candidate_root() {
        let mut db = TestDb::with_project();
        let root_dir = tempfile::tempdir().expect("root should be created");
        let root = camino::Utf8Path::from_path(root_dir.path())
            .expect("temp path should be utf8")
            .to_owned();
        let python = executable_python(&root);
        let pythonpath = vec![root.join("src")];
        let env_vars = ProjectEnvVars::from_resolved_entries(vec![
            (
                "DJANGO_SETTINGS_MODULE".to_string(),
                "env.settings".to_string(),
            ),
            ("DJLS_TEST".to_string(), "1".to_string()),
        ])
        .expect("env vars should be valid");
        let discovery = ProjectRootDiscoverySet::new(vec![RootDiscoveryInput::new(
            &db,
            root.clone(),
            Some(Interpreter::InterpreterPath(python.as_str().to_string())),
            Some(DjangoSettingsModuleSeed::new("project.settings")),
            Vec::new(),
            pythonpath.clone(),
            env_vars.clone(),
            Vec::new(),
        )])
        .expect("discovery should be valid");
        db.project()
            .set_root_discovery(&mut db)
            .to(ProjectRootDiscovery::Ready(discovery));

        let request = runtime_enrichment_request(&db, db.project())
            .expect("request should be built from configured project facts");

        assert_eq!(request.project_root, root);
        assert_eq!(request.python, python);
        assert_eq!(
            request.django_settings_module.as_deref(),
            Some("project.settings")
        );
        assert_eq!(request.pythonpath, pythonpath);
        assert_eq!(request.env_vars, env_vars.entries().to_vec());
    }

    #[test]
    fn runtime_enrichment_request_falls_back_to_first_discovery_root_for_unmatched_candidate_root()
    {
        let mut db = TestDb::with_project();
        let root_dir = tempfile::tempdir().expect("root should be created");
        let root = camino::Utf8Path::from_path(root_dir.path())
            .expect("temp path should be utf8")
            .to_owned();
        let other_dir = tempfile::tempdir().expect("other root should be created");
        let other_root = camino::Utf8Path::from_path(other_dir.path())
            .expect("temp path should be utf8")
            .to_owned();
        let python = executable_python(&root);
        let discovery = ProjectRootDiscoverySet::new(vec![RootDiscoveryInput::new(
            &db,
            root.clone(),
            Some(Interpreter::InterpreterPath(python.as_str().to_string())),
            None,
            vec![DjangoEnvironmentSeed::from_settings_module(
                Some("external".to_string()),
                DjangoSettingsModuleSeed::new("external.settings"),
                Some(other_root),
            )],
            Vec::new(),
            ProjectEnvVars::default(),
            Vec::new(),
        )])
        .expect("discovery should be valid");
        db.project()
            .set_root_discovery(&mut db)
            .to(ProjectRootDiscovery::Ready(discovery));

        let request = runtime_enrichment_request(&db, db.project())
            .expect("request should fall back to the first discovered root");

        assert_eq!(request.project_root, root);
        assert_eq!(request.python, python);
        assert_eq!(
            request.django_settings_module.as_deref(),
            Some("external.settings")
        );
    }

    #[test]
    fn enrichment_provider_translates_inspector_enrichment_to_project_enrichment() {
        let inspector_enrichment = InspectorEnrichment {
            template_libraries: BTreeMap::from([(
                "ui".to_string(),
                "blog.templatetags.ui".to_string(),
            )]),
        };

        let crate::ProjectEnrichment::Fresh(template_libraries) = inspector_enrichment.into()
        else {
            panic!("inspector enrichment should produce fresh project enrichment");
        };

        assert_eq!(
            template_libraries.get(&LibraryName::parse("ui").unwrap()),
            Some(&PyModuleName::parse("blog.templatetags.ui").unwrap())
        );
    }

    #[test]
    fn enrichment_provider_translates_failure_to_typed_issue() {
        let enrichment =
            crate::ProjectEnrichment::Unresolved(crate::ProjectEnrichmentIssue::InspectorFailed(
                crate::InspectorFailureKind::SubprocessFailed { status: None },
            ));

        assert!(matches!(
            enrichment,
            crate::ProjectEnrichment::Unresolved(crate::ProjectEnrichmentIssue::InspectorFailed(_))
        ));
    }

    #[test]
    fn enrichment_provider_owns_embedded_inspector_asset() {
        assert!(!InspectorZipapp::BYTES.is_empty());
    }
}

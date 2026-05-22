use std::collections::BTreeMap;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;

use camino::Utf8PathBuf;
use serde::Deserialize;
use serde_json::json;
use tempfile::NamedTempFile;

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
    template_dirs: Vec<Utf8PathBuf>,
    #[serde(default)]
    template_libraries: BTreeMap<String, String>,
}

impl InspectorEnrichment {
    pub(crate) fn into_draft(self) -> djls_project::ProjectEnrichmentDraft {
        djls_project::ProjectEnrichmentDraft::Fresh(djls_project::ProjectEnrichmentHints::new(
            self.template_dirs,
            self.template_libraries,
            Vec::new(),
            djls_project::DeepExtractionHints::default(),
        ))
    }
}

#[tracing::instrument(
    level = "info",
    skip_all,
    fields(
        outcome,
        project_root = %request.project_root,
        python = %request.python,
        django_settings_module = ?request.django_settings_module,
        pythonpath_entries = request.pythonpath.len(),
        env_var_count = request.env_vars.len(),
        template_dir_count,
        template_library_count,
        status,
    )
)]
pub(crate) fn load_runtime_enrichment(
    request: &RuntimeEnrichmentRequest,
) -> djls_project::ProjectEnrichmentDraft {
    let result = (|| {
        let mut process = InspectorCommand::for_request(request)?.spawn()?;

        process.write_queries()?;
        let enrichment = process.read_enrichment()?;
        process.wait()?;

        Ok(enrichment)
    })();

    match result {
        Ok(enrichment) => {
            tracing::Span::current().record("outcome", "fresh");
            enrichment.into_draft()
        }
        Err(kind) => {
            tracing::Span::current().record("outcome", "failed");
            tracing::warn!(failure = ?kind, "Runtime enrichment provider failed");
            djls_project::ProjectEnrichmentDraft::Failed {
                issue: djls_project::ProjectEnrichmentIssue::InspectorFailed { kind },
            }
        }
    }
}

struct InspectorZipapp(NamedTempFile);

impl InspectorZipapp {
    const BYTES: &'static [u8] = include_bytes!(concat!(env!("OUT_DIR"), "/djls_inspector.pyz"));

    fn create() -> Result<Self, djls_project::InspectorFailureKind> {
        let mut file = NamedTempFile::with_prefix("djls_inspector_")
            .map_err(|_| djls_project::InspectorFailureKind::SubprocessFailed { status: None })?;
        file.write_all(Self::BYTES)
            .map_err(|_| djls_project::InspectorFailureKind::SubprocessFailed { status: None })?;
        file.flush()
            .map_err(|_| djls_project::InspectorFailureKind::SubprocessFailed { status: None })?;
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
    ) -> Result<Self, djls_project::InspectorFailureKind> {
        let zipapp = InspectorZipapp::create()?;
        let mut command = Command::new(request.python.as_std_path());
        command
            .arg(zipapp.path())
            .current_dir(request.project_root.as_std_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
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
            .map_err(|_| djls_project::InspectorFailureKind::SubprocessFailed { status: None })?;
            command.env("PYTHONPATH", joined);
        }
        for (key, value) in &request.env_vars {
            command.env(key, value);
        }
        Ok(Self { command, zipapp })
    }

    fn spawn(mut self) -> Result<InspectorProcess, djls_project::InspectorFailureKind> {
        let child = self
            .command
            .spawn()
            .map_err(|_| djls_project::InspectorFailureKind::SubprocessFailed { status: None })?;
        Ok(InspectorProcess {
            child,
            _zipapp: self.zipapp,
        })
    }
}

struct InspectorProcess {
    child: Child,
    _zipapp: InspectorZipapp,
}

impl InspectorProcess {
    fn write_queries(&mut self) -> Result<(), djls_project::InspectorFailureKind> {
        let stdin = self
            .child
            .stdin
            .as_mut()
            .ok_or(djls_project::InspectorFailureKind::SubprocessFailed { status: None })?;
        writeln!(stdin, "{}", json!({ "query": "template_dirs" }))
            .map_err(|_| djls_project::InspectorFailureKind::SubprocessFailed { status: None })?;
        writeln!(stdin, "{}", json!({ "query": "template_libraries" }))
            .map_err(|_| djls_project::InspectorFailureKind::SubprocessFailed { status: None })?;
        drop(self.child.stdin.take());
        Ok(())
    }

    fn read_enrichment(
        &mut self,
    ) -> Result<InspectorEnrichment, djls_project::InspectorFailureKind> {
        let stdout = self
            .child
            .stdout
            .take()
            .ok_or(djls_project::InspectorFailureKind::SubprocessFailed { status: None })?;
        let mut lines = BufReader::new(stdout).lines();
        let template_dirs: InspectorTemplateDirs = parse_response_line(lines.next())?;
        let template_libraries: InspectorTemplateLibraries = parse_response_line(lines.next())?;

        let span = tracing::Span::current();
        span.record("template_dir_count", template_dirs.dirs.len());
        span.record("template_library_count", template_libraries.libraries.len());

        Ok(InspectorEnrichment {
            template_dirs: template_dirs.dirs,
            template_libraries: template_libraries.libraries,
        })
    }

    fn wait(mut self) -> Result<(), djls_project::InspectorFailureKind> {
        let status = self
            .child
            .wait()
            .map_err(|_| djls_project::InspectorFailureKind::SubprocessFailed { status: None })?;
        tracing::Span::current().record("status", tracing::field::debug(status.code()));
        if status.success() {
            Ok(())
        } else {
            Err(djls_project::InspectorFailureKind::SubprocessFailed {
                status: status.code(),
            })
        }
    }
}

fn parse_response_line<T: for<'de> Deserialize<'de>>(
    line: Option<std::io::Result<String>>,
) -> Result<T, djls_project::InspectorFailureKind> {
    let line = line
        .ok_or(djls_project::InspectorFailureKind::InvalidJson)?
        .map_err(|_| djls_project::InspectorFailureKind::SubprocessFailed { status: None })?;
    let response: InspectorResponse<T> =
        serde_json::from_str(&line).map_err(|_| djls_project::InspectorFailureKind::InvalidJson)?;
    if response.ok {
        response
            .data
            .ok_or(djls_project::InspectorFailureKind::InvalidJson)
    } else {
        Err(djls_project::InspectorFailureKind::SubprocessFailed { status: None })
    }
}

#[derive(Deserialize)]
struct InspectorResponse<T> {
    ok: bool,
    data: Option<T>,
}

#[derive(Deserialize)]
struct InspectorTemplateDirs {
    dirs: Vec<Utf8PathBuf>,
}

#[derive(Deserialize)]
struct InspectorTemplateLibraries {
    libraries: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enrichment_provider_translates_inspector_enrichment_to_project_draft() {
        let inspector_enrichment = InspectorEnrichment {
            template_dirs: vec![Utf8PathBuf::from("/workspace/templates")],
            template_libraries: BTreeMap::from([(
                "ui".to_string(),
                "blog.templatetags.ui".to_string(),
            )]),
        };

        let djls_project::ProjectEnrichmentDraft::Fresh(hints) = inspector_enrichment.into_draft()
        else {
            panic!("inspector enrichment should produce fresh enrichment draft");
        };

        assert_eq!(
            hints.runtime_template_dirs(),
            &[Utf8PathBuf::from("/workspace/templates")]
        );
        assert_eq!(
            hints.runtime_template_libraries().get("ui"),
            Some(&"blog.templatetags.ui".to_string())
        );
    }

    #[test]
    fn enrichment_provider_translates_failure_to_typed_issue() {
        let draft = djls_project::ProjectEnrichmentDraft::Failed {
            issue: djls_project::ProjectEnrichmentIssue::InspectorFailed {
                kind: djls_project::InspectorFailureKind::SubprocessFailed { status: None },
            },
        };

        assert!(matches!(
            draft,
            djls_project::ProjectEnrichmentDraft::Failed {
                issue: djls_project::ProjectEnrichmentIssue::InspectorFailed { .. }
            }
        ));
    }

    #[test]
    fn enrichment_provider_owns_embedded_inspector_asset() {
        assert!(!InspectorZipapp::BYTES.is_empty());
    }
}

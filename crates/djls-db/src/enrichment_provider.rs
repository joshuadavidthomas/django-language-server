use std::collections::BTreeMap;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::process::Command;
use std::process::Stdio;

use camino::Utf8PathBuf;
use serde::Deserialize;
use serde_json::json;
use tempfile::NamedTempFile;

const INSPECTOR_PYZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/djls_inspector.pyz"));

pub(crate) fn embedded_inspector_bytes() -> &'static [u8] {
    INSPECTOR_PYZ
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeEnrichmentRequest {
    pub(crate) python: Utf8PathBuf,
    pub(crate) project_root: Utf8PathBuf,
    pub(crate) django_settings_module: Option<String>,
    pub(crate) pythonpath: Vec<Utf8PathBuf>,
    pub(crate) env_vars: Vec<(String, String)>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct RuntimeEnrichmentDto {
    #[serde(default)]
    template_dirs: Vec<Utf8PathBuf>,
    #[serde(default)]
    template_libraries: BTreeMap<String, String>,
}

impl RuntimeEnrichmentDto {
    pub(crate) fn into_draft(self) -> djls_project::ProjectEnrichmentDraft {
        djls_project::ProjectEnrichmentDraft::Fresh(djls_project::ProjectEnrichmentHints::new(
            self.template_dirs,
            self.template_libraries,
            Vec::new(),
            djls_project::DeepExtractionHints::default(),
        ))
    }
}

pub(crate) fn load_runtime_enrichment(
    request: RuntimeEnrichmentRequest,
) -> djls_project::ProjectEnrichmentDraft {
    match query_runtime_enrichment(&request) {
        Ok(dto) => dto.into_draft(),
        Err(kind) => djls_project::ProjectEnrichmentDraft::Failed {
            issue: djls_project::ProjectEnrichmentIssue::InspectorFailed { kind },
        },
    }
}

fn query_runtime_enrichment(
    request: &RuntimeEnrichmentRequest,
) -> Result<RuntimeEnrichmentDto, djls_project::InspectorFailureKind> {
    let mut zipapp = NamedTempFile::with_prefix("djls_inspector_")
        .map_err(|_| djls_project::InspectorFailureKind::SubprocessFailed { status: None })?;
    zipapp
        .write_all(embedded_inspector_bytes())
        .map_err(|_| djls_project::InspectorFailureKind::SubprocessFailed { status: None })?;
    zipapp
        .flush()
        .map_err(|_| djls_project::InspectorFailureKind::SubprocessFailed { status: None })?;

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

    let mut child = command
        .spawn()
        .map_err(|_| djls_project::InspectorFailureKind::SubprocessFailed { status: None })?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or(djls_project::InspectorFailureKind::SubprocessFailed { status: None })?;
        writeln!(stdin, "{}", json!({ "query": "template_dirs" }))
            .map_err(|_| djls_project::InspectorFailureKind::SubprocessFailed { status: None })?;
        writeln!(stdin, "{}", json!({ "query": "template_libraries" }))
            .map_err(|_| djls_project::InspectorFailureKind::SubprocessFailed { status: None })?;
    }
    drop(child.stdin.take());

    let stdout = child
        .stdout
        .take()
        .ok_or(djls_project::InspectorFailureKind::SubprocessFailed { status: None })?;
    let mut lines = BufReader::new(stdout).lines();
    let template_dirs: TemplateDirsDto = parse_response_line(lines.next())?;
    let template_libraries: TemplateLibrariesDto = parse_response_line(lines.next())?;

    let status = child
        .wait()
        .map_err(|_| djls_project::InspectorFailureKind::SubprocessFailed { status: None })?;
    if !status.success() {
        return Err(djls_project::InspectorFailureKind::SubprocessFailed {
            status: status.code(),
        });
    }

    Ok(RuntimeEnrichmentDto {
        template_dirs: template_dirs.dirs,
        template_libraries: template_libraries.libraries,
    })
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
struct TemplateDirsDto {
    dirs: Vec<Utf8PathBuf>,
}

#[derive(Deserialize)]
struct TemplateLibrariesDto {
    libraries: BTreeMap<String, String>,
}

#[cfg(test)]
pub(crate) fn inspector_failure(error: impl Into<String>) -> djls_project::ProjectEnrichmentDraft {
    let error = error.into();
    let kind = if error.trim().is_empty() {
        djls_project::InspectorFailureKind::InvalidJson
    } else {
        djls_project::InspectorFailureKind::SubprocessFailed { status: None }
    };
    djls_project::ProjectEnrichmentDraft::Failed {
        issue: djls_project::ProjectEnrichmentIssue::InspectorFailed { kind },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enrichment_provider_translates_runtime_dto_to_project_draft() {
        let dto = RuntimeEnrichmentDto {
            template_dirs: vec![Utf8PathBuf::from("/workspace/templates")],
            template_libraries: BTreeMap::from([(
                "ui".to_string(),
                "blog.templatetags.ui".to_string(),
            )]),
        };

        let djls_project::ProjectEnrichmentDraft::Fresh(hints) = dto.into_draft() else {
            panic!("runtime dto should produce fresh enrichment draft");
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
        let draft = inspector_failure("boom");

        assert!(matches!(
            draft,
            djls_project::ProjectEnrichmentDraft::Failed {
                issue: djls_project::ProjectEnrichmentIssue::InspectorFailed { .. }
            }
        ));
    }

    #[test]
    fn enrichment_provider_owns_embedded_inspector_asset() {
        assert!(!embedded_inspector_bytes().is_empty());
    }
}

//! Real corpus projects opened with their own Python environments, the way the server does.
//!
//! Each project is a corpus checkout paired with the environment `corpus environment sync`
//! builds from its upstream requirements. Opening one runs the server's reload stages against
//! the real filesystem; see [`RealProject::open`].

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_ide::collect_diagnostics;
use djls_ide::prime_template_library_products;
use djls_ide::warm_cache_phases;
use djls_lsp::AnalysisHost;
use djls_lsp::Workspace;
use djls_project::run_django_discovery;
use djls_source::FileKind;
use djls_source::PositionEncoding;
use djls_source::path_to_file;
use djls_testing::Corpus;
use djls_testing::CorpusEnvironmentProject;
use tower_lsp_server::ls_types;

/// Corpus projects the `CodSpeed` benchmarks open with their real environments.
///
/// `djangoproject.com` sits near the corpus median for templates, model modules, and tag
/// modules; `netbox` is the large application. [`RealProject::load`] accepts any manifest
/// repository with an environment.
pub const REAL_PROJECTS: &[&str] = &["djangoproject.com", "netbox"];

#[derive(Debug, thiserror::Error)]
pub enum RealProjectError {
    #[error("{0}")]
    Corpus(String),
    #[error("real project {name} has no templates under {root}")]
    NoTemplates { name: String, root: Utf8PathBuf },
    #[error("invalid real project settings for {name}: {message}")]
    Settings { name: String, message: String },
}

/// A corpus checkout, its environment, and the djls settings a user would configure for it.
pub struct RealProject {
    name: String,
    project_root: Utf8PathBuf,
    /// Project templates as absolute paths, sorted.
    pub templates: Vec<Utf8PathBuf>,
    initialization_options: serde_json::Value,
    settings: Settings,
}

impl RealProject {
    /// Load the corpus project `name` with its environment, `None` when the corpus or the
    /// environment is not synchronized. Any manifest repository with an environment works.
    pub fn load(name: &str) -> Result<Option<Self>, RealProjectError> {
        if !Corpus::is_available() {
            return Ok(None);
        }
        let corpus =
            Corpus::require().map_err(|error| RealProjectError::Corpus(format!("{error:#}")))?;
        if !corpus.environment_is_synced(name) {
            return Ok(None);
        }
        Self::from_corpus(&corpus, name).map(Some)
    }

    fn from_corpus(corpus: &Corpus, name: &str) -> Result<Self, RealProjectError> {
        let project = corpus
            .environment_project(name)
            .map_err(|error| RealProjectError::Corpus(format!("{error:#}")))?;
        let CorpusEnvironmentProject {
            checkout_root,
            project_root,
            environment_root,
            django_settings_module,
            source_roots,
        } = project;

        // Files under `templates/` outside docs and tests, keeping the extensions `djls check`
        // treats as templates.
        let templates: Vec<_> = corpus
            .templates_in(&checkout_root)
            .into_iter()
            .filter(|path| FileKind::is_template(path))
            .collect();
        if templates.is_empty() {
            return Err(RealProjectError::NoTemplates {
                name: name.to_string(),
                root: checkout_root,
            });
        }

        // Client initialization options carry the configuration a user would put in
        // `djls.toml`; passing them explicitly keeps user and project config files out.
        let initialization_options = serde_json::json!({
            "venv_path": environment_root,
            "django_settings_module": django_settings_module,
            "pythonpath": source_roots,
        });
        let settings = serde_json::from_value(initialization_options.clone()).map_err(|error| {
            RealProjectError::Settings {
                name: name.to_string(),
                message: error.to_string(),
            }
        })?;

        Ok(Self {
            name: name.to_string(),
            project_root,
            templates,
            initialization_options,
            settings,
        })
    }

    fn initialize_params(&self) -> ls_types::InitializeParams {
        ls_types::InitializeParams {
            initialization_options: Some(self.initialization_options.clone()),
            ..Default::default()
        }
    }
}

impl RealProject {
    /// An empty host for this project, as the server builds on `initialize`: a root-only
    /// Project before settings apply.
    #[must_use]
    pub fn host(&self) -> AnalysisHost {
        AnalysisHost::new(
            Workspace::new(),
            Some(&self.project_root),
            &self.initialize_params(),
        )
    }

    /// Open the project on `host` the way the server's reload does: apply settings, run
    /// Django Discovery, prime Template Libraries, then warm the IDE caches.
    pub fn open(&self, host: &mut AnalysisHost) {
        host.db_mut().apply_project_settings(self.settings.clone());
        crate::require_some(
            "real project database has a Project",
            crate::require(
                "run real project Django Discovery",
                run_django_discovery(host.db_mut()),
            ),
        );
        crate::require_some(
            "prime real project Template Libraries",
            prime_template_library_products(host.db()),
        );
        for phase in warm_cache_phases() {
            crate::require_some(
                format_args!("warm real project IDE cache {:?}", phase.progress().message),
                phase.run(host.db()).count(),
            );
        }
    }

    /// Compute LSP diagnostics for every project template and return how many there are.
    #[must_use]
    pub fn check_templates(&self, host: &AnalysisHost) -> usize {
        validate_templates(host, &self.templates)
    }
}

fn validate_templates(host: &AnalysisHost, templates: &[Utf8PathBuf]) -> usize {
    let db = host.db();
    let encoding = host.client_info().position_encoding();
    templates
        .iter()
        .map(|path| {
            let file = crate::require(
                format_args!("index real project template {path}"),
                path_to_file(db, path),
            );
            diagnostics_len(host, file, encoding, path)
        })
        .sum()
}

fn diagnostics_len(
    host: &AnalysisHost,
    file: djls_source::File,
    encoding: PositionEncoding,
    path: &Utf8Path,
) -> usize {
    crate::require_some(
        format_args!("real project template {path} is eligible for diagnostics"),
        collect_diagnostics(host.db(), file, encoding),
    )
    .len()
}

/// A warm project with one template open in the editor, ready for keystrokes.
pub struct Keystroke {
    host: AnalysisHost,
    path: Utf8PathBuf,
    version: i32,
    position: ls_types::Position,
    inserted: bool,
}

impl Keystroke {
    /// Open `project` as the server does, open its keystroke template as an editor buffer, and
    /// compute that buffer's diagnostics once.
    #[must_use]
    pub fn open(project: &RealProject) -> Self {
        let (path, text, offset) = crate::require_some(
            format_args!("{} has a template with a markup-only line", project.name),
            keystroke_target(&project.templates),
        );
        let line_start = text[..offset].rfind('\n').map_or(0, |newline| newline + 1);
        let position = ls_types::Position {
            line: count_u32(text[..line_start].matches('\n').count()),
            character: count_u32(text[line_start..offset].encode_utf16().count()),
        };

        // The server reloads and warms its caches on startup; templates are diagnosed only
        // as they open.
        let mut host = project.host();
        project.open(&mut host);
        let uri = crate::require_some(
            format_args!("keystroke template {path} has a file URI"),
            ls_types::Uri::from_file_path(path.as_std_path()),
        );
        host.open_document(
            &path,
            &ls_types::TextDocumentItem {
                uri,
                language_id: "django-html".to_string(),
                version: 0,
                text,
            },
            None,
        );
        let keystroke = Self {
            host,
            path,
            version: 0,
            position,
            inserted: false,
        };
        keystroke.diagnostics();
        keystroke
    }

    /// Insert or delete one character after the anchor, alternating, then recompute the
    /// buffer's diagnostics the way the server's diagnostics publisher does.
    pub fn type_and_check(&mut self) -> usize {
        let end = if self.inserted {
            ls_types::Position {
                character: self.position.character + 1,
                ..self.position
            }
        } else {
            self.position
        };
        let text = if self.inserted { "" } else { "x" };
        self.inserted = !self.inserted;
        self.version += 1;
        crate::require_some(
            format_args!("edit open keystroke template {}", self.path),
            self.host.update_document(
                &self.path,
                self.version,
                vec![ls_types::TextDocumentContentChangeEvent {
                    range: Some(ls_types::Range {
                        start: self.position,
                        end,
                    }),
                    range_length: None,
                    text: text.to_string(),
                }],
                None,
            ),
        );
        self.diagnostics()
    }

    fn diagnostics(&self) -> usize {
        let snapshot = self.host.snapshot();
        let file = crate::require(
            format_args!("index keystroke template {}", self.path),
            path_to_file(snapshot.db(), &self.path),
        );
        crate::require_some(
            format_args!(
                "keystroke template {} is eligible for diagnostics",
                self.path
            ),
            collect_diagnostics(
                snapshot.db(),
                file,
                snapshot.client_info().position_encoding(),
            ),
        )
        .len()
    }
}

fn count_u32(count: usize) -> u32 {
    crate::require("keystroke position fits in u32", u32::try_from(count))
}

/// The template a keystroke edits and the byte offset the character goes in.
///
/// The largest project template, by bytes, with a markup-only line: a line that starts
/// with an HTML tag (not a comment, script, or style) and has no template syntax. The character goes at the end of the first
/// such line, so it lands in HTML text. Choosing by rule keeps every corpus project usable.
fn keystroke_target(templates: &[Utf8PathBuf]) -> Option<(Utf8PathBuf, String, usize)> {
    templates
        .iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(path).ok()?;
            let offset = markup_line_end(&text)?;
            Some((path.clone(), text, offset))
        })
        .max_by(|(left_path, left, _), (right_path, right, _)| {
            left.len()
                .cmp(&right.len())
                .then_with(|| right_path.cmp(left_path))
        })
}

fn markup_line_end(text: &str) -> Option<usize> {
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        let trimmed = content.trim_start();
        let markup = trimmed.starts_with('<')
            && !["<!", "<script", "<style"]
                .iter()
                .any(|prefix| trimmed.starts_with(prefix))
            && !content.contains('{');
        if markup {
            return Some(start + content.len());
        }
        start += line.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::markup_line_end;

    #[test]
    fn keystroke_goes_at_the_end_of_the_first_markup_only_line() {
        let text = concat!(
            "{% extends \"base.html\" %}\n",
            "<!-- comment -->\n",
            "  <script>\n",
            "  <a href=\"{% url 'home' %}\">Home</a>\n",
            "  <div class=\"row\">\r\n",
            "  <p>later</p>\n",
        );
        let offset = markup_line_end(text).expect("text has a markup-only line");

        assert!(text[..offset].ends_with("\n  <div class=\"row\">"));
        assert_eq!(&text[offset..offset + 2], "\r\n");
        assert_eq!(markup_line_end("{% block content %}{% endblock %}\n"), None);
    }
}

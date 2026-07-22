use std::fmt;
use std::fs;
use std::io;
use std::sync::Arc;
use std::sync::OnceLock;

use camino::Utf8Component;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_testing::Corpus;

const TEMPLATE_ROOT: &str = "/templates";

#[must_use]
fn template_path(relative: &Utf8Path) -> Utf8PathBuf {
    Utf8Path::new(TEMPLATE_ROOT).join(relative)
}

#[derive(Clone)]
pub struct Fixture {
    pub label: String,
    pub path: Utf8PathBuf,
    pub source: String,
}

impl fmt::Display for Fixture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

pub type ValidationErrorFixture = Fixture;

pub fn template_fixtures() -> Result<&'static [Fixture], FixtureLoadError> {
    static FIXTURES: OnceLock<Result<Vec<Fixture>, FixtureLoadError>> = OnceLock::new();
    match FIXTURES.get_or_init(|| {
        load_fixtures("django", &["html", "htm", "txt", "xml"], "template")
            .map(|fixtures| fixtures.into_iter().map(map_template_fixture).collect())
    }) {
        Ok(fixtures) => Ok(fixtures.as_slice()),
        Err(error) => Err(error.clone()),
    }
}

pub fn validation_error_fixtures() -> Result<&'static [Fixture], FixtureLoadError> {
    static FIXTURES: OnceLock<Result<Vec<Fixture>, FixtureLoadError>> = OnceLock::new();
    match FIXTURES.get_or_init(|| {
        load_fixtures(
            "diagnostics",
            &["html", "htm", "txt", "xml"],
            "validation error template",
        )
        .map(|fixtures| fixtures.into_iter().map(map_template_fixture).collect())
    }) {
        Ok(fixtures) => Ok(fixtures.as_slice()),
        Err(error) => Err(error.clone()),
    }
}

fn map_template_fixture(mut fixture: Fixture) -> Fixture {
    fixture.path = template_path(Utf8Path::new(&fixture.label));
    fixture
}

pub fn python_fixtures() -> Result<&'static [Fixture], FixtureLoadError> {
    static FIXTURES: OnceLock<Result<Vec<Fixture>, FixtureLoadError>> = OnceLock::new();
    match FIXTURES.get_or_init(|| load_fixtures("python", &["py"], "Python")) {
        Ok(fixtures) => Ok(fixtures.as_slice()),
        Err(error) => Err(error.clone()),
    }
}

pub fn model_fixtures() -> Result<&'static [Fixture], FixtureLoadError> {
    static FIXTURES: OnceLock<Result<Vec<Fixture>, FixtureLoadError>> = OnceLock::new();
    match FIXTURES.get_or_init(|| load_fixtures("models", &["py"], "model")) {
        Ok(fixtures) => Ok(fixtures.as_slice()),
        Err(error) => Err(error.clone()),
    }
}

/// Derive a collision-free synthetic module name for a corpus model file.
///
/// The full-corpus benchmark merges files from many repositories into one graph. Encode the corpus
/// entry and every relative path segment so repository versions stay distinct and directory names
/// that are not Python identifiers remain valid benchmark inputs.
#[must_use]
pub fn model_benchmark_module_name(file: &Utf8Path) -> String {
    let components = file
        .components()
        .filter_map(|component| match component {
            Utf8Component::Normal(segment) => Some(segment),
            Utf8Component::Prefix(_)
            | Utf8Component::RootDir
            | Utf8Component::CurDir
            | Utf8Component::ParentDir => None,
        })
        .collect::<Vec<_>>();
    let start = components
        .iter()
        .position(|component| *component == "repos")
        .map_or(0, |repos| repos + 1);
    let last = components.len().saturating_sub(1);

    components[start..]
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            let segment = if index + start == last {
                segment.strip_suffix(".py").unwrap_or(segment)
            } else {
                segment
            };
            encode_model_module_segment(segment)
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn encode_model_module_segment(segment: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut encoded = String::with_capacity(1 + segment.len() * 2);
    encoded.push('m');
    for byte in segment.bytes() {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

pub(crate) fn crate_root() -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_WORKSPACE_DIR")).join("crates/djls-bench")
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum FixtureLoadError {
    #[error("failed to load {kind} fixtures: {source}")]
    Read {
        kind: &'static str,
        #[source]
        source: Arc<io::Error>,
    },
    #[error("no {kind} files discovered under {root}")]
    Empty {
        kind: &'static str,
        root: Utf8PathBuf,
    },
}

#[derive(Debug)]
pub struct CorpusTemplates {
    #[cfg(test)]
    pub(crate) discovered_file_count: usize,
    pub files: Vec<(Utf8PathBuf, String)>,
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum CorpusLoadError {
    #[error("benchmark corpus is invalid or stale: {message}")]
    Invalid { message: String },
    #[error("Django package is missing under corpus root {corpus_root}")]
    MissingDjangoPackage { corpus_root: Utf8PathBuf },
    #[error("no {selection} corpus templates discovered under {selection_root}")]
    NoTemplates {
        selection: &'static str,
        selection_root: Utf8PathBuf,
    },
    #[error("corpus template {path} is outside corpus root {corpus_root}")]
    OutsideCorpusRoot {
        path: Utf8PathBuf,
        corpus_root: Utf8PathBuf,
    },
    #[error("failed to read corpus template {path}: {source}")]
    ReadTemplate {
        path: Utf8PathBuf,
        #[source]
        source: Arc<io::Error>,
    },
}

fn read_corpus_templates(
    corpus_root: &Utf8Path,
    selection: &'static str,
    selection_root: &Utf8Path,
    mut paths: Vec<Utf8PathBuf>,
) -> Result<CorpusTemplates, CorpusLoadError> {
    paths.sort();
    if paths.is_empty() {
        return Err(CorpusLoadError::NoTemplates {
            selection,
            selection_root: selection_root.to_path_buf(),
        });
    }

    let discovered_file_count = paths.len();
    let mut files = Vec::with_capacity(discovered_file_count);
    for path in paths {
        let relative = path
            .strip_prefix(corpus_root)
            .map_err(|_strip_prefix_error| CorpusLoadError::OutsideCorpusRoot {
                path: path.clone(),
                corpus_root: corpus_root.to_path_buf(),
            })?;
        let source = fs::read_to_string(path.as_std_path()).map_err(|error| {
            CorpusLoadError::ReadTemplate {
                path: path.clone(),
                source: Arc::new(error),
            }
        })?;
        files.push((template_path(relative), source));
    }

    Ok(CorpusTemplates {
        #[cfg(test)]
        discovered_file_count,
        files,
    })
}

fn load_corpus_templates(
    selection: &'static str,
    get_selection: impl FnOnce(&Corpus) -> Result<(Utf8PathBuf, Vec<Utf8PathBuf>), CorpusLoadError>,
) -> Result<Option<CorpusTemplates>, CorpusLoadError> {
    if !Corpus::is_available() {
        return Ok(None);
    }

    let corpus = Corpus::require().map_err(|error| CorpusLoadError::Invalid {
        message: error.to_string(),
    })?;
    let (selection_root, paths) = get_selection(&corpus)?;
    read_corpus_templates(corpus.root(), selection, &selection_root, paths).map(Some)
}

pub fn django_corpus_templates() -> Result<Option<&'static CorpusTemplates>, CorpusLoadError> {
    static CORPUS: OnceLock<Result<Option<CorpusTemplates>, CorpusLoadError>> = OnceLock::new();
    match CORPUS.get_or_init(|| {
        load_corpus_templates("Django", |corpus| {
            let django_dir = corpus.latest_package("django").ok_or_else(|| {
                CorpusLoadError::MissingDjangoPackage {
                    corpus_root: corpus.root().to_path_buf(),
                }
            })?;
            let paths = corpus.templates_in(&django_dir);
            Ok((django_dir, paths))
        })
    }) {
        Ok(corpus) => Ok(corpus.as_ref()),
        Err(error) => Err(error.clone()),
    }
}

pub fn full_corpus_templates() -> Result<Option<&'static CorpusTemplates>, CorpusLoadError> {
    static CORPUS: OnceLock<Result<Option<CorpusTemplates>, CorpusLoadError>> = OnceLock::new();
    match CORPUS.get_or_init(|| {
        load_corpus_templates("full", |corpus| {
            let corpus_root = corpus.root().to_path_buf();
            let paths = corpus.templates_in(&corpus_root);
            Ok((corpus_root, paths))
        })
    }) {
        Ok(corpus) => Ok(corpus.as_ref()),
        Err(error) => Err(error.clone()),
    }
}

fn load_fixtures(
    subdir: &str,
    extensions: &[&str],
    kind: &'static str,
) -> Result<Vec<Fixture>, FixtureLoadError> {
    let root = crate_root().join("fixtures").join(subdir);

    let mut raw = Vec::new();
    collect_files(root.as_path(), root.as_path(), extensions, &mut raw).map_err(|source| {
        FixtureLoadError::Read {
            kind,
            source: Arc::new(source),
        }
    })?;

    raw.sort_by(|a, b| a.0.cmp(&b.0));

    let fixtures: Vec<Fixture> = raw
        .into_iter()
        .map(|(label, path, source)| Fixture {
            label,
            path,
            source,
        })
        .collect();

    if fixtures.is_empty() {
        return Err(FixtureLoadError::Empty { kind, root });
    }

    Ok(fixtures)
}

fn collect_files(
    root: &Utf8Path,
    dir: &Utf8Path,
    extensions: &[&str],
    fixtures: &mut Vec<(String, Utf8PathBuf, String)>,
) -> io::Result<()> {
    for entry in fs::read_dir(dir.as_std_path())? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        let utf8_path = Utf8PathBuf::from_path_buf(path.clone()).map_err(|original| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("path {} is not valid UTF-8", original.display()),
            )
        })?;

        if file_type.is_dir() {
            collect_files(root, utf8_path.as_path(), extensions, fixtures)?;
            continue;
        }

        if file_type.is_file()
            && utf8_path
                .extension()
                .is_some_and(|ext| extensions.contains(&ext))
        {
            let source = fs::read_to_string(utf8_path.as_std_path())?;
            let relative = utf8_path.strip_prefix(root).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{utf8_path} is not under {root}: {err}"),
                )
            })?;

            fixtures.push((relative.to_string(), utf8_path, source));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::fmt::Write as _;

    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_project::PythonModuleName;
    use djls_testing::Corpus;
    use insta::assert_yaml_snapshot;
    use serde::Serialize;
    use sha2::Digest;
    use sha2::Sha256;

    use super::CorpusLoadError;
    use super::Fixture;
    use super::django_corpus_templates;
    use super::full_corpus_templates;
    use super::model_benchmark_module_name;
    use super::model_fixtures;
    use super::python_fixtures;
    use super::read_corpus_templates;
    use super::template_fixtures;
    use super::validation_error_fixtures;
    use crate::bench_corpus_is_required;

    #[derive(Serialize)]
    struct FixtureSetSnapshot {
        file_count: usize,
        total_bytes: usize,
        files: Vec<FixtureIdentitySnapshot>,
    }

    #[derive(Serialize)]
    struct FixtureIdentitySnapshot {
        path: String,
        bytes: usize,
        source_sha256: String,
    }

    fn sha256(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        let mut output = String::with_capacity(digest.len() * 2);
        for byte in digest {
            assert!(
                write!(output, "{byte:02x}").is_ok(),
                "writing a SHA-256 digest to a String should succeed"
            );
        }
        output
    }

    fn fixture_set_snapshot(fixtures: &[Fixture]) -> FixtureSetSnapshot {
        let mut files: Vec<_> = fixtures
            .iter()
            .map(|fixture| FixtureIdentitySnapshot {
                path: fixture.label.clone(),
                bytes: fixture.source.len(),
                source_sha256: sha256(fixture.source.as_bytes()),
            })
            .collect();
        files.sort_by(|left, right| left.path.cmp(&right.path));

        FixtureSetSnapshot {
            file_count: fixtures.len(),
            total_bytes: fixtures.iter().map(|fixture| fixture.source.len()).sum(),
            files,
        }
    }

    #[test]
    fn fixture_identities_are_stable() {
        assert_yaml_snapshot!(
            "fixture_identity_templates",
            fixture_set_snapshot(
                template_fixtures().expect("template benchmark fixtures should load")
            )
        );
        assert_yaml_snapshot!(
            "fixture_identity_validation_errors",
            fixture_set_snapshot(
                validation_error_fixtures()
                    .expect("validation error benchmark fixtures should load")
            )
        );
        assert_yaml_snapshot!(
            "fixture_identity_python",
            fixture_set_snapshot(python_fixtures().expect("Python benchmark fixtures should load"))
        );
        assert_yaml_snapshot!(
            "fixture_identity_models",
            fixture_set_snapshot(model_fixtures().expect("model benchmark fixtures should load"))
        );
    }

    #[test]
    fn model_benchmark_module_names_include_and_encode_corpus_entries() {
        let path = Utf8Path::new(
            ".corpus/repos/regular-django/examples/regular-django/example/users/models.py",
        );
        let other_entry = Utf8Path::new(
            ".corpus/repos/regular_django/examples/regular-django/example/users/models.py",
        );

        let name = model_benchmark_module_name(path);
        assert!(
            PythonModuleName::parse(&name).is_ok(),
            "synthetic benchmark module name should be valid: {name}"
        );
        assert_ne!(name, model_benchmark_module_name(other_entry));
    }

    #[test]
    fn synchronized_corpus_model_module_names_are_unique_and_valid() {
        let required = bench_corpus_is_required();
        if !Corpus::is_available() {
            assert!(!required, "model benchmark corpus is not synchronized");
            eprintln!("model benchmark corpus is not synchronized; skipping module-name check");
            return;
        }

        let corpus = Corpus::require().expect("benchmark corpus should load");
        let paths = corpus.model_files_in(corpus.root());
        let mut names = HashSet::with_capacity(paths.len());
        for path in paths {
            let name = model_benchmark_module_name(&path);
            assert!(
                PythonModuleName::parse(&name).is_ok(),
                "synthetic module name for {path} should be valid: {name}"
            );
            assert!(
                names.insert(name),
                "model benchmark module names must be unique"
            );
        }
    }

    #[test]
    fn available_corpus_selection_without_templates_is_an_error() {
        let corpus_root = Utf8Path::new("/corpus");
        let selection_root = corpus_root.join("repos/django-6.0");

        let error = read_corpus_templates(corpus_root, "Django", &selection_root, Vec::new())
            .expect_err("an available empty Django corpus selection should fail");

        assert!(matches!(
            error,
            CorpusLoadError::NoTemplates {
                selection: "Django",
                selection_root: root,
            } if root == selection_root
        ));
    }

    #[test]
    fn corpus_path_errors_retain_the_offending_paths() {
        let corpus_root = Utf8Path::new("/corpus");
        let outside_path = Utf8PathBuf::from("/elsewhere/template.html");
        let error =
            read_corpus_templates(corpus_root, "full", corpus_root, vec![outside_path.clone()])
                .expect_err("a template outside the corpus root should fail corpus loading");
        assert!(matches!(
            error,
            CorpusLoadError::OutsideCorpusRoot {
                path,
                corpus_root: root,
            } if path == outside_path && root == corpus_root
        ));

        let unreadable_path = corpus_root.join("missing.html");
        let error = read_corpus_templates(
            corpus_root,
            "full",
            corpus_root,
            vec![unreadable_path.clone()],
        )
        .expect_err("an unreadable corpus template should fail corpus loading");
        assert!(
            matches!(
                error,
                CorpusLoadError::ReadTemplate { path, .. } if path == unreadable_path
            ),
            "read error should retain the unreadable path"
        );
    }

    #[test]
    fn corpus_loader_synchronizes_every_discovered_template() {
        let required = bench_corpus_is_required();
        for (name, corpus) in [
            ("Django", django_corpus_templates()),
            ("full", full_corpus_templates()),
        ] {
            let corpus = corpus.expect("benchmark corpus should load");
            let Some(corpus) = corpus else {
                assert!(!required, "{name} benchmark corpus is not synchronized");
                eprintln!("{name} benchmark corpus is not synchronized; skipping loader check");
                continue;
            };
            assert_eq!(corpus.discovered_file_count, corpus.files.len());
        }
    }
}

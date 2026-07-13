use std::fmt;
use std::fs;
use std::io;
use std::sync::OnceLock;

use camino::Utf8Path;
use camino::Utf8PathBuf;

const TEMPLATE_ROOT: &str = "/templates";

#[must_use]
pub fn template_path(relative: &Utf8Path) -> Utf8PathBuf {
    Utf8Path::new(TEMPLATE_ROOT).join(relative)
}

#[derive(Clone)]
pub struct Fixture {
    pub label: String,
    pub path: Utf8PathBuf,
    pub source: String,
}

/// Stable identity and size contract for a benchmark fixture set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixtureDigest {
    pub file_count: usize,
    pub total_bytes: usize,
    pub sorted_paths: Vec<Utf8PathBuf>,
}

impl FixtureDigest {
    #[must_use]
    pub fn from_fixtures(fixtures: &[Fixture]) -> Self {
        let mut sorted_paths: Vec<_> = fixtures
            .iter()
            .map(|fixture| fixture.path.clone())
            .collect();
        sorted_paths.sort();

        Self {
            file_count: fixtures.len(),
            total_bytes: fixtures.iter().map(|fixture| fixture.source.len()).sum(),
            sorted_paths,
        }
    }
}

impl fmt::Display for Fixture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

pub type ValidationErrorFixture = Fixture;

pub fn template_fixtures() -> &'static [Fixture] {
    static FIXTURES: OnceLock<Vec<Fixture>> = OnceLock::new();
    FIXTURES
        .get_or_init(|| {
            load_fixtures("django", &["html", "htm", "txt", "xml"], "template")
                .into_iter()
                .map(map_template_fixture)
                .collect()
        })
        .as_slice()
}

pub fn validation_error_fixtures() -> &'static [Fixture] {
    static FIXTURES: OnceLock<Vec<Fixture>> = OnceLock::new();
    FIXTURES
        .get_or_init(|| {
            load_fixtures(
                "diagnostics",
                &["html", "htm", "txt", "xml"],
                "validation error template",
            )
            .into_iter()
            .map(map_template_fixture)
            .collect()
        })
        .as_slice()
}

#[must_use]
pub fn template_fixture_digest() -> FixtureDigest {
    FixtureDigest::from_fixtures(template_fixtures())
}

#[must_use]
pub fn validation_error_fixture_digest() -> FixtureDigest {
    FixtureDigest::from_fixtures(validation_error_fixtures())
}

fn map_template_fixture(mut fixture: Fixture) -> Fixture {
    fixture.path = template_path(Utf8Path::new(&fixture.label));
    fixture
}

pub fn python_fixtures() -> &'static [Fixture] {
    static FIXTURES: OnceLock<Vec<Fixture>> = OnceLock::new();
    FIXTURES
        .get_or_init(|| load_fixtures("python", &["py"], "Python"))
        .as_slice()
}

pub fn model_fixtures() -> &'static [Fixture] {
    static FIXTURES: OnceLock<Vec<Fixture>> = OnceLock::new();
    FIXTURES
        .get_or_init(|| load_fixtures("models", &["py"], "model"))
        .as_slice()
}

pub(crate) fn crate_root() -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_WORKSPACE_DIR")).join("crates/djls-bench")
}

fn load_fixtures(subdir: &str, extensions: &[&str], kind: &str) -> Vec<Fixture> {
    let root = crate_root().join("fixtures").join(subdir);

    let mut raw = Vec::new();
    collect_files(root.as_path(), root.as_path(), extensions, &mut raw)
        .unwrap_or_else(|err| panic!("failed to load {kind} fixtures: {err}"));

    raw.sort_by(|a, b| a.0.cmp(&b.0));

    let fixtures: Vec<Fixture> = raw
        .into_iter()
        .map(|(label, path, source)| Fixture {
            label,
            path,
            source,
        })
        .collect();

    assert!(
        !fixtures.is_empty(),
        "no {kind} files discovered under {root}"
    );

    fixtures
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
    use super::*;

    #[test]
    fn template_fixture_contract_is_stable() {
        assert_eq!(
            template_fixture_digest(),
            FixtureDigest {
                file_count: 6,
                total_bytes: 30_105,
                sorted_paths: vec![
                    "/templates/large/stress_test.html".into(),
                    "/templates/large/views_technical_500.html".into(),
                    "/templates/medium/admin_login.html".into(),
                    "/templates/medium/nested_blocks.html".into(),
                    "/templates/small/dense_tags.html".into(),
                    "/templates/small/forms_widgets_input.html".into(),
                ],
            }
        );
    }

    #[test]
    fn validation_error_fixture_contract_is_stable() {
        assert_eq!(
            validation_error_fixture_digest(),
            FixtureDigest {
                file_count: 8,
                total_bytes: 80_053,
                sorted_paths: vec![
                    "/templates/large/dense_validation_errors.html".into(),
                    "/templates/large/sparse_validation_errors.html".into(),
                    "/templates/medium/mixed_validation_errors.html".into(),
                    "/templates/medium/nested_block_errors.html".into(),
                    "/templates/small/invalid_filter_arity.html".into(),
                    "/templates/small/invalid_tag_args.html".into(),
                    "/templates/small/mismatched_block.html".into(),
                    "/templates/small/unknown_symbols.html".into(),
                ],
            }
        );
    }
}

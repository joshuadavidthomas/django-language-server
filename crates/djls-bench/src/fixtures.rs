use std::fmt;
use std::fs;
use std::io;
use std::sync::OnceLock;

use camino::Utf8Path;
use camino::Utf8PathBuf;

#[derive(Clone)]
pub struct TemplateFixture {
    pub label: String,
    pub path: Utf8PathBuf,
    pub source: String,
}

impl fmt::Display for TemplateFixture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

#[derive(Clone)]
pub struct PythonFixture {
    pub label: String,
    pub path: Utf8PathBuf,
    pub source: String,
}

impl fmt::Display for PythonFixture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

pub fn template_fixtures() -> &'static [TemplateFixture] {
    static FIXTURES: OnceLock<Vec<TemplateFixture>> = OnceLock::new();
    FIXTURES.get_or_init(load_template_fixtures).as_slice()
}

pub fn python_fixtures() -> &'static [PythonFixture] {
    static FIXTURES: OnceLock<Vec<PythonFixture>> = OnceLock::new();
    FIXTURES.get_or_init(load_python_fixtures).as_slice()
}

pub(crate) fn crate_root() -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_WORKSPACE_DIR")).join("crates/djls-bench")
}

fn load_template_fixtures() -> Vec<TemplateFixture> {
    let root = crate_root().join("fixtures").join("django");

    let mut fixtures = Vec::new();
    collect_files(
        root.as_path(),
        root.as_path(),
        &["html", "htm", "txt", "xml"],
        &mut fixtures,
    )
    .unwrap_or_else(|err| panic!("failed to load template fixtures: {err}"));

    let fixtures = fixtures
        .into_iter()
        .map(|(label, path, source)| TemplateFixture {
            label,
            path,
            source,
        })
        .collect::<Vec<_>>();

    assert!(!fixtures.is_empty(), "no templates discovered under {root}");

    fixtures
}

fn load_python_fixtures() -> Vec<PythonFixture> {
    let root = crate_root().join("fixtures").join("python");

    let mut fixtures = Vec::new();
    collect_files(root.as_path(), root.as_path(), &["py"], &mut fixtures)
        .unwrap_or_else(|err| panic!("failed to load Python fixtures: {err}"));

    let fixtures = fixtures
        .into_iter()
        .map(|(label, path, source)| PythonFixture {
            label,
            path,
            source,
        })
        .collect::<Vec<_>>();

    assert!(
        !fixtures.is_empty(),
        "no Python files discovered under {root}"
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

    fixtures.sort_by(|a, b| a.0.cmp(&b.0));

    Ok(())
}

use std::fmt;
use std::fs;
use std::io;
use std::sync::OnceLock;

use camino::Utf8Path;
use camino::Utf8PathBuf;

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

pub type TemplateFixture = Fixture;
pub type PythonFixture = Fixture;
pub type ModelFixture = Fixture;

pub fn template_fixtures() -> &'static [Fixture] {
    static FIXTURES: OnceLock<Vec<Fixture>> = OnceLock::new();
    FIXTURES
        .get_or_init(|| load_fixtures("django", &["html", "htm", "txt", "xml"], "template"))
        .as_slice()
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

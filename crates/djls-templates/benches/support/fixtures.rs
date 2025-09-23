use std::fmt;
use std::fs;
use std::io;
use std::sync::OnceLock;

use camino::Utf8Path;
use camino::Utf8PathBuf;

#[derive(Clone)]
pub(crate) struct TemplateFixture {
    pub label: String,
    pub path: Utf8PathBuf,
    pub source: String,
}

impl fmt::Display for TemplateFixture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

pub(crate) fn template_fixtures() -> &'static [TemplateFixture] {
    static FIXTURES: OnceLock<Vec<TemplateFixture>> = OnceLock::new();
    FIXTURES.get_or_init(load_template_fixtures).as_slice()
}

fn load_template_fixtures() -> Vec<TemplateFixture> {
    let workspace_root = option_env!("CARGO_WORKSPACE_DIR")
        .and_then(|value| if value.is_empty() { None } else { Some(value) })
        .map_or_else(
            || panic!("CARGO_WORKSPACE_DIR must be configured for benchmarks"),
            Utf8PathBuf::from,
        );
    let template_root = workspace_root.join("tests/project");

    let mut fixtures = Vec::new();
    collect_template_files(
        template_root.as_path(),
        template_root.as_path(),
        &mut fixtures,
    )
    .unwrap_or_else(|err| panic!("failed to load template fixtures: {err}"));

    fixtures.sort_by(|a, b| a.label.cmp(&b.label));
    assert!(
        !fixtures.is_empty(),
        "no templates discovered under {template_root}",
    );

    fixtures
}

fn collect_template_files(
    root: &Utf8Path,
    dir: &Utf8Path,
    fixtures: &mut Vec<TemplateFixture>,
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
            collect_template_files(root, utf8_path.as_path(), fixtures)?;
            continue;
        }

        if file_type.is_file()
            && matches!(utf8_path.extension(), Some("html" | "htm" | "txt" | "xml"))
        {
            let source = fs::read_to_string(utf8_path.as_std_path())?;
            let relative = utf8_path.strip_prefix(root).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{utf8_path} is not under {root}: {err}"),
                )
            })?;

            fixtures.push(TemplateFixture {
                label: relative.to_string(),
                path: utf8_path,
                source,
            });
        }
    }

    Ok(())
}

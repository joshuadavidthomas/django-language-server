use std::fs;

use anyhow::Context as _;
use anyhow::bail;
use camino::Utf8Path;
use camino::Utf8PathBuf;

use crate::Corpus;

pub struct VendorSpecFixturesOptions {
    pub check: bool,
    pub output_dir: Option<Utf8PathBuf>,
}

pub fn vendor_spec_fixtures(options: VendorSpecFixturesOptions) -> anyhow::Result<()> {
    let corpus = Corpus::require();
    let output_dir = options.output_dir.unwrap_or_else(default_spec_fixture_dir);

    if !options.check {
        fs::create_dir_all(output_dir.as_std_path())
            .with_context(|| format!("failed to create {output_dir}"))?;
    }

    let mut stale = Vec::new();
    for fixture in SPEC_FIXTURES {
        let content = render_fixture(&corpus, fixture)?;
        let output_path = output_dir.join(fixture.output_file);
        if options.check {
            check_fixture(&output_path, &content, &mut stale)?;
        } else {
            write_fixture(&output_path, &content)?;
        }
    }

    if !stale.is_empty() {
        bail!(
            "vendored spec fixtures are out of date:\n  {}\nrun `just corpus vendor-spec-fixtures` to update them",
            stale.join("\n  ")
        );
    }

    Ok(())
}

fn default_spec_fixture_dir() -> Utf8PathBuf {
    Utf8Path::new(env!("CARGO_MANIFEST_DIR")).join("../djls-project/src/specs/testdata")
}

fn write_fixture(path: &Utf8Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent.as_std_path())
            .with_context(|| format!("failed to create {parent}"))?;
    }
    fs::write(path.as_std_path(), content).with_context(|| format!("failed to write {path}"))
}

fn check_fixture(path: &Utf8Path, expected: &str, stale: &mut Vec<String>) -> anyhow::Result<()> {
    match fs::read_to_string(path.as_std_path()) {
        Ok(actual) if actual == expected => Ok(()),
        Ok(_) => {
            stale.push(path.to_string());
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            stale.push(path.to_string());
            Ok(())
        }
        Err(error) => Err(error).with_context(|| format!("failed to read {path}")),
    }
}

fn render_fixture(corpus: &Corpus, fixture: &SpecFixture) -> anyhow::Result<String> {
    let package_dir = corpus.latest_package(fixture.package).ok_or_else(|| {
        anyhow::anyhow!(
            "synced corpus package `{}` not found; run `just corpus sync`",
            fixture.package
        )
    })?;
    let entry_name = package_dir.file_name().ok_or_else(|| {
        anyhow::anyhow!("corpus package path has no final component: {package_dir}")
    })?;
    let source_path = package_dir.join(fixture.relative_path);
    let source = fs::read_to_string(source_path.as_std_path())
        .with_context(|| format!("failed to read {source_path}"))?;

    let mut chunks = vec![fixture_header(entry_name, fixture.relative_path)];
    for chunk in fixture.chunks {
        chunks.push(render_chunk(&source, chunk).with_context(|| {
            format!(
                "failed to extract `{}` from {entry_name}/{}",
                chunk.description(),
                fixture.relative_path
            )
        })?);
    }

    Ok(format!("{}\n", chunks.join("\n\n").trim_end()))
}

fn fixture_header(entry_name: &str, relative_path: &str) -> String {
    format!(
        "# Vendored unit-test fixture.\n# Corpus: {entry_name}/{relative_path}\n# Keep snippets minimal: live corpus drift is covered by crates/djls-project/tests/corpus*.rs.\n\nfrom django import template\n\nregister = template.Library()"
    )
}

fn render_chunk(source: &str, chunk: &FixtureChunk) -> anyhow::Result<String> {
    match chunk {
        FixtureChunk::TopLevelItem(name) => extract_top_level_item(source, name)
            .ok_or_else(|| anyhow::anyhow!("top-level item `{name}` not found")),
        FixtureChunk::SourceLine(line) => extract_source_line(source, line)
            .ok_or_else(|| anyhow::anyhow!("source line `{line}` not found")),
    }
}

fn extract_source_line(source: &str, needle: &str) -> Option<String> {
    source
        .lines()
        .find(|line| line.trim() == needle)
        .map(str::to_owned)
}

fn extract_top_level_item(source: &str, name: &str) -> Option<String> {
    let lines: Vec<_> = source.lines().collect();
    let item_index = lines
        .iter()
        .position(|line| is_top_level_item_line(line, name))?;

    let mut start = item_index;
    while start > 0 && lines[start - 1].starts_with('@') {
        start -= 1;
    }

    let end = lines
        .iter()
        .enumerate()
        .skip(item_index + 1)
        .find_map(|(index, line)| {
            if is_top_level_boundary(line) {
                Some(index)
            } else {
                None
            }
        })
        .unwrap_or(lines.len());

    Some(lines[start..end].join("\n").trim_end().to_owned())
}

fn is_top_level_boundary(line: &str) -> bool {
    !line.trim().is_empty() && !line.starts_with(char::is_whitespace)
}

fn is_top_level_item_line(line: &str, name: &str) -> bool {
    if line.starts_with(char::is_whitespace) {
        return false;
    }

    let function = format!("def {name}(");
    let async_function = format!("async def {name}(");
    let class_with_base = format!("class {name}(");
    let class_without_base = format!("class {name}:");

    line.starts_with(&function)
        || line.starts_with(&async_function)
        || line.starts_with(&class_with_base)
        || line.starts_with(&class_without_base)
}

struct SpecFixture {
    output_file: &'static str,
    package: &'static str,
    relative_path: &'static str,
    chunks: &'static [FixtureChunk],
}

enum FixtureChunk {
    TopLevelItem(&'static str),
    SourceLine(&'static str),
}

impl FixtureChunk {
    fn description(&self) -> &'static str {
        match self {
            FixtureChunk::TopLevelItem(name) => name,
            FixtureChunk::SourceLine(line) => line,
        }
    }
}

const SPEC_FIXTURES: &[SpecFixture] = &[
    SpecFixture {
        output_file: "django_defaulttags.py",
        package: "django",
        relative_path: "django/template/defaulttags.py",
        chunks: &[
            FixtureChunk::TopLevelItem("autoescape"),
            FixtureChunk::TopLevelItem("comment"),
            FixtureChunk::TopLevelItem("cycle"),
            FixtureChunk::TopLevelItem("do_for"),
            FixtureChunk::TopLevelItem("do_if"),
            FixtureChunk::TopLevelItem("now"),
            FixtureChunk::TopLevelItem("partial_func"),
            FixtureChunk::TopLevelItem("partialdef_func"),
            FixtureChunk::TopLevelItem("regroup"),
            FixtureChunk::TopLevelItem("spaceless"),
            FixtureChunk::TopLevelItem("templatetag"),
            FixtureChunk::TopLevelItem("url"),
            FixtureChunk::TopLevelItem("verbatim"),
            FixtureChunk::TopLevelItem("widthratio"),
            FixtureChunk::TopLevelItem("querystring"),
        ],
    },
    SpecFixture {
        output_file: "django_defaultfilters.py",
        package: "django",
        relative_path: "django/template/defaultfilters.py",
        chunks: &[
            FixtureChunk::TopLevelItem("add"),
            FixtureChunk::TopLevelItem("addslashes"),
            FixtureChunk::TopLevelItem("cut"),
            FixtureChunk::TopLevelItem("date"),
            FixtureChunk::TopLevelItem("default"),
            FixtureChunk::TopLevelItem("escapejs_filter"),
            FixtureChunk::TopLevelItem("floatformat"),
            FixtureChunk::TopLevelItem("lower"),
            FixtureChunk::TopLevelItem("title"),
            FixtureChunk::TopLevelItem("upper"),
        ],
    },
    SpecFixture {
        output_file: "django_loader_tags.py",
        package: "django",
        relative_path: "django/template/loader_tags.py",
        chunks: &[
            FixtureChunk::TopLevelItem("do_block"),
            FixtureChunk::TopLevelItem("do_include"),
        ],
    },
    SpecFixture {
        output_file: "django_i18n.py",
        package: "django",
        relative_path: "django/templatetags/i18n.py",
        chunks: &[
            FixtureChunk::TopLevelItem("do_block_translate"),
            FixtureChunk::TopLevelItem("do_translate"),
        ],
    },
    SpecFixture {
        output_file: "django_tz.py",
        package: "django",
        relative_path: "django/templatetags/tz.py",
        chunks: &[
            FixtureChunk::TopLevelItem("get_current_timezone_tag"),
            FixtureChunk::TopLevelItem("localtime_tag"),
            FixtureChunk::TopLevelItem("timezone_tag"),
        ],
    },
    SpecFixture {
        output_file: "django_admin_urls.py",
        package: "django",
        relative_path: "django/contrib/admin/templatetags/admin_urls.py",
        chunks: &[FixtureChunk::TopLevelItem("add_preserved_filters")],
    },
    SpecFixture {
        output_file: "django_custom.py",
        package: "django",
        relative_path: "tests/template_tests/templatetags/custom.py",
        chunks: &[
            FixtureChunk::TopLevelItem("div"),
            FixtureChunk::TopLevelItem("no_params"),
            FixtureChunk::TopLevelItem("no_params_with_context"),
            FixtureChunk::TopLevelItem("one_param"),
            FixtureChunk::TopLevelItem("simple_one_default"),
            FixtureChunk::TopLevelItem("simple_two_params"),
        ],
    },
    SpecFixture {
        output_file: "django_inclusion.py",
        package: "django",
        relative_path: "tests/template_tests/templatetags/inclusion.py",
        chunks: &[
            FixtureChunk::TopLevelItem("inclusion_no_params"),
            FixtureChunk::TopLevelItem("inclusion_no_params_with_context"),
            FixtureChunk::TopLevelItem("inclusion_one_default"),
            FixtureChunk::TopLevelItem("inclusion_one_param"),
        ],
    },
    SpecFixture {
        output_file: "django_testtags.py",
        package: "django",
        relative_path: "tests/template_tests/templatetags/testtags.py",
        chunks: &[
            FixtureChunk::TopLevelItem("echo"),
            FixtureChunk::SourceLine("register.tag(\"other_echo\", echo)"),
        ],
    },
    SpecFixture {
        output_file: "allauth_tags.py",
        package: "django-allauth",
        relative_path: "allauth/templatetags/allauth.py",
        chunks: &[
            FixtureChunk::TopLevelItem("parse_tag"),
            FixtureChunk::TopLevelItem("do_element"),
        ],
    },
    SpecFixture {
        output_file: "wagtailadmin_tags.py",
        package: "wagtail",
        relative_path: "wagtail/admin/templatetags/wagtailadmin_tags.py",
        chunks: &[
            FixtureChunk::SourceLine("register.filter(\"intcomma\", intcomma)"),
            FixtureChunk::TopLevelItem("DialogNode"),
            FixtureChunk::SourceLine("register.tag(\"dialog\", DialogNode.handle)"),
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_decorated_top_level_function() {
        let source = r#"
@register.tag("demo")
def do_demo(parser, token):
    bits = token.split_contents()
    return bits

class Other:
    pass
"#;

        let extracted = extract_top_level_item(source.trim_start(), "do_demo").unwrap();
        assert_eq!(
            extracted,
            "@register.tag(\"demo\")\ndef do_demo(parser, token):\n    bits = token.split_contents()\n    return bits"
        );
    }

    #[test]
    fn extracts_top_level_class() {
        let source = r#"
class DialogNode(BlockInclusionNode):
    template = "dialog.html"

    def get_context_data(self, parent_context):
        return {}

register.tag("dialog", DialogNode.handle)
"#;

        let extracted = extract_top_level_item(source.trim_start(), "DialogNode").unwrap();
        assert_eq!(
            extracted,
            "class DialogNode(BlockInclusionNode):\n    template = \"dialog.html\"\n\n    def get_context_data(self, parent_context):\n        return {}"
        );
    }
}

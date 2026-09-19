use std::collections::BTreeSet;
use std::fs;

use anyhow::Context as _;
use anyhow::bail;
use camino::Utf8Path;
use camino::Utf8PathBuf;

use crate::Corpus;
use crate::fixtures::source_fixture_root;

pub struct VendorSpecFixturesOptions {
    pub check: bool,
    pub output_dir: Option<Utf8PathBuf>,
}

pub fn vendor_spec_fixtures(options: VendorSpecFixturesOptions) -> anyhow::Result<()> {
    let corpus = Corpus::require()?;
    let source_output_dir = options
        .output_dir
        .as_ref()
        .map_or_else(source_fixture_root, |path| path.join("source"));
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

    for fixture in SOURCE_FIXTURES {
        let package_dir = corpus.latest_package(fixture.repo).ok_or_else(|| {
            anyhow::anyhow!(
                "synced corpus repository `{}` not found; run `just corpus sync`",
                fixture.repo
            )
        })?;
        let source_path = package_dir.join(fixture.relative_path);
        let content = fs::read(source_path.as_std_path())
            .with_context(|| format!("failed to read {source_path}"))?;
        let output_path = source_output_dir
            .join(fixture.repo)
            .join(fixture.relative_path);
        if options.check {
            check_fixture(&output_path, &content, &mut stale)?;
        } else {
            write_fixture(&output_path, &content)?;
        }
    }

    for repo in SOURCE_FIXTURES
        .iter()
        .map(|fixture| fixture.repo)
        .collect::<BTreeSet<_>>()
    {
        let license_path = Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("licenses")
            .join(repo);
        let content = fs::read(license_path.as_std_path())
            .with_context(|| format!("failed to read {license_path}"))?;
        let output_path = source_output_dir.join(repo).join("LICENSE");
        if options.check {
            check_fixture(&output_path, &content, &mut stale)?;
        } else {
            write_fixture(&output_path, &content)?;
        }
    }

    if !stale.is_empty() {
        bail!(
            "vendored fixtures are out of date:\n  {}\nrun `just corpus vendor-spec-fixtures` to update them",
            stale.join("\n  ")
        );
    }

    Ok(())
}

fn default_spec_fixture_dir() -> Utf8PathBuf {
    Utf8Path::new(env!("CARGO_MANIFEST_DIR")).join("../djls-project/src/templates/tags/testdata")
}

fn write_fixture(path: &Utf8Path, content: impl AsRef<[u8]>) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent.as_std_path())
            .with_context(|| format!("failed to create {parent}"))?;
    }
    fs::write(path.as_std_path(), content).with_context(|| format!("failed to write {path}"))
}

fn check_fixture(
    path: &Utf8Path,
    expected: impl AsRef<[u8]>,
    stale: &mut Vec<String>,
) -> anyhow::Result<()> {
    match fs::read(path.as_std_path()) {
        Ok(actual) if actual == expected.as_ref() => Ok(()),
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

struct SourceFixture {
    repo: &'static str,
    relative_path: &'static str,
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

macro_rules! source_fixtures {
    ($($repo:literal => [$($path:literal),+ $(,)?]),+ $(,)?) => {
        &[$($(SourceFixture { repo: $repo, relative_path: $path }),+),+]
    };
}

const SOURCE_FIXTURES: &[SourceFixture] = source_fixtures![
    "django-5.2" => [
        "django/__init__.py",
        "django/contrib/__init__.py",
        "django/contrib/admin/__init__.py",
        "django/contrib/admin/templatetags/__init__.py",
        "django/contrib/admin/templatetags/admin_list.py",
        "django/contrib/admin/templatetags/admin_modify.py",
        "django/contrib/admin/templatetags/admin_urls.py",
        "django/contrib/admin/templatetags/base.py",
        "django/contrib/admin/templatetags/log.py",
        "django/contrib/admin/templates/admin/base.html",
        "django/contrib/auth/__init__.py",
        "django/contrib/auth/templates/registration/password_reset_subject.txt",
        "django/contrib/contenttypes/__init__.py",
        "django/contrib/flatpages/__init__.py",
        "django/contrib/flatpages/templatetags/__init__.py",
        "django/contrib/flatpages/templatetags/flatpages.py",
        "django/contrib/humanize/__init__.py",
        "django/contrib/humanize/templatetags/__init__.py",
        "django/contrib/humanize/templatetags/humanize.py",
        "django/contrib/messages/__init__.py",
        "django/contrib/sessions/__init__.py",
        "django/contrib/staticfiles/__init__.py",
        "django/template/__init__.py",
        "django/template/base.py",
        "django/template/context.py",
        "django/template/defaultfilters.py",
        "django/template/defaulttags.py",
        "django/template/library.py",
        "django/template/loader_tags.py",
        "django/templatetags/__init__.py",
        "django/templatetags/cache.py",
        "django/templatetags/i18n.py",
        "django/templatetags/l10n.py",
        "django/templatetags/static.py",
        "django/templatetags/tz.py",
        "tests/check_framework/__init__.py",
        "tests/check_framework/template_test_apps/__init__.py",
        "tests/check_framework/template_test_apps/different_tags_app/__init__.py",
        "tests/check_framework/template_test_apps/different_tags_app/templatetags/__init__.py",
        "tests/check_framework/template_test_apps/different_tags_app/templatetags/different_tags.py",
        "tests/check_framework/template_test_apps/same_tags_app_1/__init__.py",
        "tests/check_framework/template_test_apps/same_tags_app_1/templatetags/__init__.py",
        "tests/check_framework/template_test_apps/same_tags_app_1/templatetags/same_tags.py",
        "tests/check_framework/template_test_apps/same_tags_app_2/__init__.py",
        "tests/check_framework/template_test_apps/same_tags_app_2/templatetags/__init__.py",
        "tests/check_framework/template_test_apps/same_tags_app_2/templatetags/same_tags.py",
        "tests/forms_tests/__init__.py",
        "tests/forms_tests/templatetags/__init__.py",
        "tests/forms_tests/templatetags/tags.py",
        "tests/template_backends/__init__.py",
        "tests/template_backends/apps/__init__.py",
        "tests/template_backends/apps/good/__init__.py",
        "tests/template_backends/apps/good/templatetags/__init__.py",
        "tests/template_backends/apps/good/templatetags/empty.py",
        "tests/template_backends/apps/good/templatetags/good_tags.py",
        "tests/template_backends/apps/good/templatetags/override.py",
        "tests/template_backends/apps/importerror/__init__.py",
        "tests/template_backends/apps/importerror/templatetags/__init__.py",
        "tests/template_backends/apps/importerror/templatetags/broken_tags.py",
        "tests/template_tests/__init__.py",
        "tests/template_tests/templatetags/__init__.py",
        "tests/template_tests/templatetags/bad_tag.py",
        "tests/template_tests/templatetags/custom.py",
        "tests/template_tests/templatetags/inclusion.py",
        "tests/template_tests/templatetags/tag_27584.py",
        "tests/template_tests/templatetags/testtags.py",
        "tests/view_tests/__init__.py",
        "tests/view_tests/templatetags/__init__.py",
        "tests/view_tests/templatetags/debugtags.py",
    ],
    "django-6.0" => ["django/template/defaultfilters.py"],
    "django-6.1" => ["django/template/defaultfilters.py"],
    "sentry" => [
        "src/sentry/templatetags/sentry_assets.py",
        "src/sentry/utils/assets.py",
    ],
    "django-pipeline" => ["pipeline/templatetags/pipeline.py"],
    "django-compressor" => ["compressor/templatetags/compress.py"],
    "django-cms" => ["cms/templatetags/cms_tags.py"],
    "pretix" => ["src/pretix/base/templatetags/eventsignal.py"],
    "django-activity-stream" => ["actstream/templatetags/activity_tags.py"],
    "django-allauth" => ["allauth/templatetags/allauth.py"],
    "django-sekizai" => ["sekizai/templatetags/sekizai_tags.py"],
];

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

        let extracted = extract_top_level_item(source.trim_start(), "do_demo")
            .expect("decorated top-level function should be extracted");
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

        let extracted = extract_top_level_item(source.trim_start(), "DialogNode")
            .expect("top-level class should be extracted");
        assert_eq!(
            extracted,
            "class DialogNode(BlockInclusionNode):\n    template = \"dialog.html\"\n\n    def get_context_data(self, parent_context):\n        return {}"
        );
    }
}

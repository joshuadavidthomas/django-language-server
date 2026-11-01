use djls_project::*;
use djls_source::ChangeEvent;
use djls_source::SourceChanges;
use djls_testing::ProjectFixture;
use djls_testing::ProjectSettings;
use djls_testing::TestDatabase;

const ROOT: &str = "/test/project";
const DJANGO_COMPONENTS_READY: &str = r#"import re
from django.apps import AppConfig

class ComponentsConfig(AppConfig):
    name = "blog"

    def ready(self):
        if app_settings.MULTILINE_TAGS:
            from django.template import base

            base.tag_re = re.compile(base.tag_re.pattern, re.DOTALL)
"#;

fn tag_regex_for(
    installed_apps: &[&str],
    files: &[(&str, &str)],
) -> Result<TemplateTagRegex, Box<dyn std::error::Error>> {
    let db = TestDatabase::new();
    let settings = ProjectSettings {
        installed_apps: installed_apps.iter().map(ToString::to_string).collect(),
        ..ProjectSettings::default()
    };
    let project = files
        .iter()
        .fold(
            ProjectFixture::new(ROOT).settings(&settings),
            |fixture, (path, source)| fixture.file(format!("{ROOT}/{path}"), *source),
        )
        .build(&db)?;
    Ok(template_tag_regex(&db, project))
}

fn patched_app(patch: &str) -> Result<TemplateTagRegex, Box<dyn std::error::Error>> {
    tag_regex_for(&["blog"], &[("blog/__init__.py", patch)])
}

#[test]
fn startup_code_without_a_patch_keeps_the_default_pattern() {
    assert_eq!(
        tag_regex_for(
            &["blog", "django.contrib.admin"],
            &[
                (
                    "blog/__init__.py",
                    "import re\nPATTERN = re.compile('x', re.DOTALL)\n"
                ),
                ("blog/models.py", "from django.db import models\n"),
            ],
        )
        .expect("fixture should build"),
        TemplateTagRegex::Default
    );
}

#[test]
fn dotall_assignments_in_common_spellings_are_recognized() {
    for patch in [
        "import re\nfrom django.template import base\nbase.tag_re = re.compile(base.tag_re.pattern, re.DOTALL)\n",
        "import re\nfrom django.template import base as template_base\ntemplate_base.tag_re = re.compile(template_base.tag_re.pattern, re.S)\n",
        "import re\nimport django.template.base\ndjango.template.base.tag_re = re.compile(django.template.base.tag_re.pattern, flags=re.DOTALL)\n",
        "import re\nimport django.template.base as b\nb.tag_re = re.compile(r'({%.*?%}|{{.*?}}|{#.*?#})', re.MULTILINE | re.DOTALL)\n",
        "from re import DOTALL, compile\nfrom django.template import base\nbase.tag_re = compile(base.tag_re.pattern, DOTALL)\n",
        "import re as regex\nfrom django.template import base\nbase.tag_re = regex.compile(base.tag_re.pattern, regex.RegexFlag.DOTALL)\n",
        "import re\nfrom django.template import base\nbase.tag_re = re.compile(r'(?s){%.*?%}|{{.*?}}|{#.*?#}')\n",
        "import re\nfrom django.template import base\nPATTERN = re.compile(base.tag_re.pattern, re.DOTALL)\nbase.tag_re = PATTERN\n",
        "import re\nfrom django.template import base\nsetattr(base, 'tag_re', re.compile(base.tag_re.pattern, re.DOTALL))\n",
        "import re\nfrom django.template import base\nbase.tag_re = re.compile(base.tag_re.pattern, 16)\n",
    ] {
        assert_eq!(
            patched_app(patch).expect("fixture should build"),
            TemplateTagRegex::DotAll,
            "{patch}"
        );
    }
}

#[test]
fn tag_re_assignments_without_proven_dotall_are_classified() {
    for (patch, expected) in [
        (
            "import re\nfrom django.template import base\nbase.tag_re = re.compile(base.tag_re.pattern, re.MULTILINE)\n",
            TemplateTagRegex::Default,
        ),
        (
            "import re\nbase = object()\nbase.tag_re = re.compile('x', re.DOTALL)\n",
            TemplateTagRegex::Default,
        ),
        (
            "import re\nfrom django.template import base\nfrom django.template import loader as base\nbase.tag_re = re.compile('x', re.DOTALL)\n",
            TemplateTagRegex::Default,
        ),
        (
            "from django.template import base\nbase.tag_re = build_pattern()\n",
            TemplateTagRegex::Unknown,
        ),
        (
            "from django.template import base\nimport re\nre = load_regex_module()\nbase.tag_re = re.compile(base.tag_re.pattern, re.DOTALL)\n",
            TemplateTagRegex::Unknown,
        ),
        (
            "import re\nfrom django.template import base\nbase.tag_re = re.compile(base.tag_re.pattern, FLAGS)\n",
            TemplateTagRegex::Unknown,
        ),
    ] {
        assert_eq!(
            patched_app(patch).expect("fixture should build"),
            expected,
            "{patch}"
        );
    }
}

#[test]
fn patches_are_found_where_django_runs_them_at_startup() {
    let patch = "import re\nfrom django.template import base\nbase.tag_re = re.compile(base.tag_re.pattern, re.DOTALL)\n";
    assert_eq!(
        tag_regex_for(&[], &[("unused_patch.py", patch)]).expect("fixture should build"),
        TemplateTagRegex::Default,
        "a module nothing imports never runs"
    );
    // The settings module itself.
    let db = TestDatabase::new();
    let settings = format!("{}{patch}", ProjectSettings::default().settings_py());
    let project = ProjectFixture::new(ROOT)
        .django_settings_module("settings")
        .file(format!("{ROOT}/settings.py"), settings)
        .build(&db)
        .expect("fixture should build");
    assert_eq!(template_tag_regex(&db, project), TemplateTagRegex::DotAll);
    // An app's `AppConfig.ready()`, in the conditional, function-local form django-components uses.
    assert_eq!(
        tag_regex_for(
            &["blog.apps.ComponentsConfig"],
            &[
                ("blog/__init__.py", ""),
                ("blog/apps.py", DJANGO_COMPONENTS_READY)
            ],
        )
        .expect("fixture should build"),
        TemplateTagRegex::DotAll
    );
    // A module the app package imports, relatively or absolutely.
    for init in [
        "from . import patches\n",
        "import blog.patches\n",
        "from .patches import *\n",
    ] {
        assert_eq!(
            tag_regex_for(
                &["blog"],
                &[("blog/__init__.py", init), ("blog/patches.py", patch)],
            )
            .expect("fixture should build"),
            TemplateTagRegex::DotAll,
            "{init}"
        );
    }
    // A module the app's `models` imports from the app package.
    assert_eq!(
        tag_regex_for(
            &["blog"],
            &[
                ("blog/__init__.py", ""),
                (
                    "blog/models.py",
                    "def setup():\n    from blog.compat import patches\n"
                ),
                ("blog/compat/__init__.py", ""),
                ("blog/compat/patches.py", patch),
            ],
        )
        .expect("fixture should build"),
        TemplateTagRegex::DotAll
    );
}

#[test]
fn startup_code_extraction_cannot_read_leaves_the_pattern_unknown() {
    assert_eq!(
        tag_regex_for(&["missing_app"], &[]).expect("fixture should build"),
        TemplateTagRegex::Unknown,
        "an installed app outside the search paths"
    );

    let db = TestDatabase::new();
    let project = ProjectFixture::new(ROOT)
        .build(&db)
        .expect("fixture should build");
    assert_eq!(
        template_tag_regex(&db, project),
        TemplateTagRegex::Unknown,
        "no Django settings module"
    );
}

#[test]
fn editing_a_startup_module_recomputes_the_pattern() {
    let mut db = TestDatabase::new();
    let settings = ProjectSettings {
        installed_apps: vec!["blog".to_string()],
        ..ProjectSettings::default()
    };
    let path = format!("{ROOT}/blog/__init__.py");
    let project = ProjectFixture::new(ROOT)
        .settings(&settings)
        .file(&path, "")
        .build(&db)
        .expect("fixture should build");
    assert_eq!(template_tag_regex(&db, project), TemplateTagRegex::Default);

    db.add_file(
        &path,
        "import re\nfrom django.template import base\nbase.tag_re = re.compile(base.tag_re.pattern, re.DOTALL)\n",
    ).expect("fixture should build");
    SourceChanges::new([ChangeEvent::ContentChanged(path.into())]).apply(&mut db);
    assert_eq!(template_tag_regex(&db, project), TemplateTagRegex::DotAll);
}

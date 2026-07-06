use djls_project::testing::django_settings;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;

#[test]
fn baseline_literal_installed_apps_and_templates() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
from pathlib import Path
BASE_DIR = Path(__file__).resolve().parent.parent
INSTALLED_APPS = [
    "django.contrib.admin",
    "blog.apps.BlogConfig",
]
TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "DIRS": [BASE_DIR / "templates", "/shared/templates"],
        "APP_DIRS": True,
        "OPTIONS": {
            "libraries": {"custom": "blog.templatetags.custom"},
            "builtins": ["django.templatetags.static"],
        },
    },
]
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn split_settings_non_star_import_degrades_imported_name() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings.local")
        .file("/proj/myproject/__init__.py", "")
        .file("/proj/myproject/settings/__init__.py", "")
        .file(
            "/proj/myproject/settings/base.py",
            r#"
INSTALLED_APPS = ["django.contrib.admin", "blog"]
"#,
        )
        .file(
            "/proj/myproject/settings/local.py",
            r#"
from .base import INSTALLED_APPS
TEMPLATES = [{"DIRS": ["templates"], "APP_DIRS": True}]
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn split_settings_star_import_resolves_base_settings() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings.local")
        .file("/proj/myproject/__init__.py", "")
        .file("/proj/myproject/settings/__init__.py", "")
        .file(
            "/proj/myproject/settings/base.py",
            r#"
from pathlib import Path
BASE_DIR = Path(__file__).resolve().parent.parent
INSTALLED_APPS = ["django.contrib.auth"]
TEMPLATES = [{"DIRS": [BASE_DIR / "templates"], "APP_DIRS": True}]
"#,
        )
        .file(
            "/proj/myproject/settings/local.py",
            r#"
from .base import *
INSTALLED_APPS += ["blog"]
TEMPLATES[0]["DIRS"].append(BASE_DIR / "local_templates")
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn composed_app_lists_via_literal_aliases_degrade() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
DJANGO_APPS = ["django.contrib.admin"]
LOCAL_APPS = ["myapp"]
INSTALLED_APPS = DJANGO_APPS + LOCAL_APPS
TEMPLATES = [{"DIRS": [], "APP_DIRS": True}]
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn duplicate_template_dirs_keys_append_values() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
INSTALLED_APPS = ["blog"]
TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "DIRS": ["first"],
        "DIRS": ["second"],
        "APP_DIRS": True,
    },
]
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn unknown_template_backend_spread_keeps_surrounding_keys() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
INSTALLED_APPS = ["blog"]
extra = {}
TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "DIRS": ["templates"],
        **extra,
        "APP_DIRS": True,
    },
]
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn unsupported_template_dirs_insert_mutation_degrades_templates() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
INSTALLED_APPS = ["blog"]
TEMPLATES = [{"DIRS": ["base"], "APP_DIRS": True}]
TEMPLATES[0]["DIRS"].insert(0, "first")
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn ambiguous_branch_alias_degrades_installed_apps() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
if FLAG:
    APPS = ["a"]
else:
    APPS = ["b"]
INSTALLED_APPS = APPS
TEMPLATES = [{"DIRS": [], "APP_DIRS": True}]
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

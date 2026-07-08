use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_project::Interpreter;
use djls_project::SearchPaths;
use djls_project::testing::django_settings;
use djls_source::Db as _;
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
fn split_settings_non_star_import_resolves_imported_setting() {
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
fn split_settings_aliased_non_star_import_feeds_installed_apps() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings.local")
        .file("/proj/myproject/__init__.py", "")
        .file("/proj/myproject/settings/__init__.py", "")
        .file(
            "/proj/myproject/settings/base.py",
            r#"
INSTALLED_APPS = ["django.contrib.auth"]
"#,
        )
        .file(
            "/proj/myproject/settings/local.py",
            r#"
from .base import INSTALLED_APPS as BASE_APPS
INSTALLED_APPS = BASE_APPS + ["blog"]
TEMPLATES = [{"DIRS": [], "APP_DIRS": True}]
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn split_settings_non_star_import_chain_resolves_imported_setting() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings.local")
        .file("/proj/myproject/__init__.py", "")
        .file("/proj/myproject/settings/__init__.py", "")
        .file(
            "/proj/myproject/settings/common.py",
            r#"
COMMON_APPS = ["django.contrib.auth"]
"#,
        )
        .file(
            "/proj/myproject/settings/base.py",
            r#"
from .common import COMMON_APPS
INSTALLED_APPS = COMMON_APPS + ["blog"]
"#,
        )
        .file(
            "/proj/myproject/settings/local.py",
            r#"
from .base import INSTALLED_APPS
TEMPLATES = [{"DIRS": [], "APP_DIRS": True}]
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn split_settings_cyclic_non_star_import_does_not_hang() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings.local")
        .file("/proj/myproject/__init__.py", "")
        .file("/proj/myproject/settings/__init__.py", "")
        .file(
            "/proj/myproject/settings/base.py",
            r#"
from .local import INSTALLED_APPS as LOCAL_APPS
INSTALLED_APPS = ["django.contrib.auth"]
"#,
        )
        .file(
            "/proj/myproject/settings/local.py",
            r#"
from .base import INSTALLED_APPS
TEMPLATES = [{"DIRS": [], "APP_DIRS": True}]
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn non_star_import_from_extra_search_path_is_not_followed() {
    let mut db = TestDatabase::new();
    db.add_file(
        "/vendor/vendor_settings.py",
        r#"
INSTALLED_APPS = ["vendor"]
"#,
    );
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/proj"),
        &Interpreter::Auto,
        &[Utf8PathBuf::from("/vendor")],
    );
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .search_paths(search_paths)
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
from vendor_settings import INSTALLED_APPS
TEMPLATES = [{"DIRS": [], "APP_DIRS": True}]
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
fn composed_app_lists_via_literal_aliases_extract() {
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
fn duplicate_template_dirs_keys_use_last_value() {
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
fn template_backend_spread_keeps_prior_keys_partial() {
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
fn unsupported_plain_call_touching_known_settings_degrades_them() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
INSTALLED_APPS = ["a"]
TEMPLATES = [{"BACKEND": "django.template.backends.django.DjangoTemplates"}]
configure(INSTALLED_APPS, TEMPLATES)
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn unsupported_attribute_call_touching_both_known_settings_degrades_both() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
INSTALLED_APPS = ["a"]
TEMPLATES = [{"BACKEND": "django.template.backends.django.DjangoTemplates"}]
helpers.configure(INSTALLED_APPS, TEMPLATES)
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn ambiguous_branch_alias_extracts_partial_installed_apps() {
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

#[test]
fn template_context_processors_literal_entries_extract() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "DIRS": [],
        "APP_DIRS": True,
        "OPTIONS": {
            "context_processors": [
                "django.template.context_processors.debug",
                "django.template.context_processors.request",
                "django.contrib.auth.context_processors.auth",
                "django.contrib.messages.context_processors.messages",
            ],
        },
    },
]
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn template_context_processors_mixed_invalid_entries_extract_partial() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
UNKNOWN_PROCESSOR = "project.context_processors.dynamic"
TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "DIRS": [],
        "APP_DIRS": True,
        "OPTIONS": {
            "context_processors": [
                "project.context_processors.site",
                42,
                UNKNOWN_PROCESSOR,
                "bad-module.processor",
                "project.context_processors.request",
            ],
        },
    },
]
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn template_context_processors_non_list_extracts_partial_backend() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "DIRS": [],
        "APP_DIRS": True,
        "OPTIONS": {"context_processors": "project.context_processors.site"},
    },
]
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn template_context_processors_conditional_assignment_keeps_both_branch_facts_partial() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
if FLAG:
    TEMPLATES = [
        {
            "BACKEND": "django.template.backends.django.DjangoTemplates",
            "DIRS": [],
            "APP_DIRS": True,
            "OPTIONS": {"context_processors": ["project.context_processors.site"]},
        },
    ]
else:
    TEMPLATES = [
        {
            "BACKEND": "django.template.backends.django.DjangoTemplates",
            "DIRS": [],
            "APP_DIRS": True,
            "OPTIONS": {"context_processors": ["project.context_processors.site"]},
        },
    ]
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn static_url_literal_extracts_originated_candidate() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
STATIC_URL = "/static/"
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn static_url_conditional_override_keeps_all_candidates_partial() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
STATIC_URL = "/static/"
if USE_CDN:
    STATIC_URL = "https://cdn.example.com/static/"
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn static_url_unknown_expression_extracts_partial_without_candidate() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
BASE_URL = "/assets/"
STATIC_URL = BASE_URL + "static/"
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn static_root_resolved_path_extracts_originated_candidate() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
from pathlib import Path
BASE_DIR = Path(__file__).resolve().parent.parent
STATIC_ROOT = BASE_DIR / "staticfiles"
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn static_root_unknown_path_extracts_unknown_candidate_partial() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
STATIC_ROOT = STATIC_BASE / "staticfiles"
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn static_root_conditional_assignment_keeps_all_candidates_partial() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
from pathlib import Path
BASE_DIR = Path(__file__).resolve().parent.parent
STATIC_ROOT = BASE_DIR / "staticfiles"
if USE_TMP:
    STATIC_ROOT = BASE_DIR / "tmp-staticfiles"
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn staticfiles_dirs_list_resolved_paths_extract() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
from pathlib import Path
BASE_DIR = Path(__file__).resolve().parent.parent
STATICFILES_DIRS = [BASE_DIR / "assets", "/shared/static"]
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn staticfiles_dirs_tuple_resolved_paths_extract() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
from pathlib import Path
BASE_DIR = Path(__file__).resolve().parent.parent
STATICFILES_DIRS = (BASE_DIR / "assets", "/shared/static")
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn staticfiles_dirs_unknown_element_extracts_unknown_partial() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r"
STATICFILES_DIRS = [STATIC_ASSETS]
",
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn staticfiles_dirs_mixed_known_unknown_elements_extract_partial() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
from pathlib import Path
BASE_DIR = Path(__file__).resolve().parent.parent
STATICFILES_DIRS = [BASE_DIR / "assets", STATIC_ASSETS, "/shared/static"]
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn staticfiles_dirs_conditional_assignment_keeps_all_candidates_partial() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
from pathlib import Path
BASE_DIR = Path(__file__).resolve().parent.parent
STATICFILES_DIRS = [BASE_DIR / "assets"]
if USE_VENDOR:
    STATICFILES_DIRS = [BASE_DIR / "vendor-assets"]
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn static_url_star_import_conditional_reassignment_keeps_base_candidate_partial() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings.local")
        .file("/proj/myproject/__init__.py", "")
        .file("/proj/myproject/settings/__init__.py", "")
        .file(
            "/proj/myproject/settings/base.py",
            r#"
STATIC_URL = "/static/"
"#,
        )
        .file(
            "/proj/myproject/settings/local.py",
            r#"
from .base import *
if USE_CDN:
    STATIC_URL = "https://cdn.example.com/static/"
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

#[test]
fn staticfiles_dirs_append_mutation_degrades_setting() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
from pathlib import Path
BASE_DIR = Path(__file__).resolve().parent.parent
STATICFILES_DIRS = [BASE_DIR / "assets"]
STATICFILES_DIRS.append(BASE_DIR / "more-assets")
"#,
        )
        .install(&mut db);

    insta::assert_yaml_snapshot!(django_settings(&db, project));
}

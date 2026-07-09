use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_project::Interpreter;
use djls_project::Project;
use djls_project::SearchPaths;
use djls_project::testing::compute_django_discovery;
use djls_project::testing::django_settings;
use djls_source::Db as _;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ExtractionStatus {
    Complete,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ParseStatus {
    Parsed,
    Unparseable,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum EvaluatedPath {
    Resolved(Utf8PathBuf),
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Originated<T> {
    value: T,
}

impl<T> Originated<T> {
    fn value(&self) -> &T {
        &self.value
    }
}

#[derive(Debug, Deserialize)]
struct SettingValues<T> {
    values: Vec<T>,
    extraction: ExtractionStatus,
}

#[derive(Debug, Deserialize)]
struct TemplateBackend {
    backend: Option<String>,
    dirs: Vec<EvaluatedPath>,
    libraries: Vec<(String, String)>,
    context_processors: Vec<Originated<String>>,
    extraction: ExtractionStatus,
}

impl TemplateBackend {
    fn is_fully_extracted(&self) -> bool {
        self.extraction == ExtractionStatus::Complete
    }
}

#[derive(Debug, Deserialize)]
struct TemplateSettings {
    backends: Vec<TemplateBackend>,
    extraction: ExtractionStatus,
}

#[derive(Debug, Deserialize)]
struct StaticFilesSettings {
    static_url: SettingValues<Originated<String>>,
    static_root: SettingValues<Originated<EvaluatedPath>>,
}

#[derive(Debug, Deserialize)]
struct ExtractedSettings {
    parse_status: ParseStatus,
    installed_apps: SettingValues<String>,
    templates: TemplateSettings,
    staticfiles: StaticFilesSettings,
}

fn extract_project(
    source: &str,
    modules: &[(&str, &str)],
) -> (TestDatabase, Project, ExtractedSettings) {
    let mut fixture = ProjectFixture::new("/project/settings")
        .django_settings_module("config.settings")
        .file("/project/settings/config/__init__.py", "")
        .file("/project/settings/config/settings.py", source);
    for (module, source) in modules {
        let path = format!("/project/settings/{}.py", module.replace('.', "/"));
        fixture = fixture.file(path, *source);
    }

    let mut db = TestDatabase::new();
    let project = fixture.install(&mut db);
    let settings = serde_json::from_value(
        serde_json::to_value(django_settings(&db, project)).expect("settings should serialize"),
    )
    .expect("settings should deserialize into the test projection");
    (db, project, settings)
}

fn extract(source: &str) -> ExtractedSettings {
    extract_project(source, &[]).2
}

fn extract_with_modules(source: &str, modules: &[(&str, &str)]) -> ExtractedSettings {
    extract_project(source, modules).2
}

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
TEMPLATES = [{"BACKEND": "django.template.backends.django.DjangoTemplates", "DIRS": ["templates"], "APP_DIRS": True}]
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
TEMPLATES = [{"BACKEND": "django.template.backends.django.DjangoTemplates", "DIRS": [], "APP_DIRS": True}]
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
TEMPLATES = [{"BACKEND": "django.template.backends.django.DjangoTemplates", "DIRS": [], "APP_DIRS": True}]
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
TEMPLATES = [{"BACKEND": "django.template.backends.django.DjangoTemplates", "DIRS": [], "APP_DIRS": True}]
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
TEMPLATES = [{"BACKEND": "django.template.backends.django.DjangoTemplates", "DIRS": [], "APP_DIRS": True}]
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
TEMPLATES = [{"BACKEND": "django.template.backends.django.DjangoTemplates", "DIRS": [BASE_DIR / "templates"], "APP_DIRS": True}]
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
TEMPLATES = [{"BACKEND": "django.template.backends.django.DjangoTemplates", "DIRS": [], "APP_DIRS": True}]
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
TEMPLATES = [{"BACKEND": "django.template.backends.django.DjangoTemplates", "DIRS": ["base"], "APP_DIRS": True}]
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
TEMPLATES = [{"BACKEND": "django.template.backends.django.DjangoTemplates", "DIRS": [], "APP_DIRS": True}]
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
fn explicit_template_backends_from_different_alternatives_remain_distinct() {
    let settings = extract(
        "if FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/a']}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/b']}]",
    );

    assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
    assert_eq!(settings.templates.backends.len(), 2);
    assert_eq!(
        settings.templates.backends[0].dirs,
        [EvaluatedPath::Resolved(Utf8PathBuf::from("/a"))]
    );
    assert_eq!(
        settings.templates.backends[1].dirs,
        [EvaluatedPath::Resolved(Utf8PathBuf::from("/b"))]
    );
}

#[test]
fn explicit_template_backends_within_one_value_remain_distinct() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/a']}, {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/b']}]",
    );

    assert_eq!(settings.templates.extraction, ExtractionStatus::Complete);
    assert_eq!(settings.templates.backends.len(), 2);
    assert_eq!(
        settings.templates.backends[0].dirs,
        [EvaluatedPath::Resolved(Utf8PathBuf::from("/a"))]
    );
    assert_eq!(
        settings.templates.backends[1].dirs,
        [EvaluatedPath::Resolved(Utf8PathBuf::from("/b"))]
    );
}

#[test]
fn template_context_processors_identical_branches_preserve_origins_and_partial_status() {
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
fn static_url_identical_conditional_assignments_preserve_origins_and_partial_status() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file(
            "/proj/myproject/settings.py",
            r#"
if USE_CDN:
    STATIC_URL = "/static/"
else:
    STATIC_URL = "/static/"
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

#[test]
fn unreachable_import_is_not_a_semantic_dependency() {
    let (db, project, settings) = extract_project(
        "if False:\n    from base import INSTALLED_APPS\nelse:\n    INSTALLED_APPS = ['local']",
        &[("base", "INSTALLED_APPS = [")],
    );
    let discovery = compute_django_discovery(&db, project);

    assert_eq!(
        discovery.file_paths(),
        [Utf8PathBuf::from("/project/settings/config/settings.py")]
    );
    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Complete
    );
    assert_eq!(settings.installed_apps.values, ["local"]);
}

#[test]
fn unreachable_elif_import_is_not_a_semantic_dependency() {
    let (db, project, settings) = extract_project(
        "if FLAG:\n    INSTALLED_APPS = ['local']\nelif False:\n    from base import INSTALLED_APPS",
        &[("base", "INSTALLED_APPS = [")],
    );
    let discovery = compute_django_discovery(&db, project);

    assert_eq!(
        discovery.file_paths(),
        [Utf8PathBuf::from("/project/settings/config/settings.py")]
    );
    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert_eq!(settings.installed_apps.values, ["local"]);
}

#[test]
fn ambiguous_branch_import_effects_are_semantic_dependencies() {
    let (db, project, settings) = extract_project(
        "if FLAG:\n    from base import INSTALLED_APPS\nelse:\n    INSTALLED_APPS = ['local']",
        &[("base", "INSTALLED_APPS = [")],
    );
    let discovery = compute_django_discovery(&db, project);

    assert_eq!(
        discovery.file_paths(),
        [
            Utf8PathBuf::from("/project/settings/base.py"),
            Utf8PathBuf::from("/project/settings/config/settings.py"),
        ]
    );
    assert_eq!(settings.parse_status, ParseStatus::Unparseable);
    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert_eq!(settings.installed_apps.values, ["local"]);
}

#[test]
fn loop_import_effects_are_dependencies_without_accepting_values() {
    let (db, project, settings) = extract_project(
        "for app in []:\n    from base import INSTALLED_APPS",
        &[("base", "INSTALLED_APPS = [")],
    );
    let discovery = compute_django_discovery(&db, project);

    assert_eq!(
        discovery.file_paths(),
        [
            Utf8PathBuf::from("/project/settings/base.py"),
            Utf8PathBuf::from("/project/settings/config/settings.py"),
        ]
    );
    assert_eq!(settings.parse_status, ParseStatus::Unparseable);
    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert!(settings.installed_apps.values.is_empty());
}

#[test]
fn loop_star_import_degrades_existing_bindings_without_accepting_values() {
    let (db, project, settings) = extract_project(
        "INSTALLED_APPS = ['local']\nfor app in PLUGINS:\n    from base import *",
        &[("base", "INSTALLED_APPS = ['base']")],
    );
    let discovery = compute_django_discovery(&db, project);

    assert_eq!(
        discovery.file_paths(),
        [
            Utf8PathBuf::from("/project/settings/base.py"),
            Utf8PathBuf::from("/project/settings/config/settings.py"),
        ]
    );
    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert_eq!(settings.installed_apps.values, ["local"]);
}

#[test]
fn loop_nested_unreachable_star_import_does_not_degrade_bindings() {
    let (db, project, settings) = extract_project(
        "INSTALLED_APPS = ['local']\nfor app in PLUGINS:\n    if False:\n        from base import *",
        &[("base", "INSTALLED_APPS = [")],
    );
    let discovery = compute_django_discovery(&db, project);

    assert_eq!(
        discovery.file_paths(),
        [Utf8PathBuf::from("/project/settings/config/settings.py")]
    );
    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Complete
    );
    assert_eq!(settings.installed_apps.values, ["local"]);
}

#[test]
fn while_false_body_import_is_not_a_semantic_dependency() {
    let (db, project, settings) = extract_project(
        "while False:\n    from base import INSTALLED_APPS\nelse:\n    INSTALLED_APPS = ['local']",
        &[("base", "INSTALLED_APPS = [")],
    );
    let discovery = compute_django_discovery(&db, project);

    assert_eq!(
        discovery.file_paths(),
        [Utf8PathBuf::from("/project/settings/config/settings.py")]
    );
    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Complete
    );
    assert_eq!(settings.installed_apps.values, ["local"]);
}

#[test]
fn literal_tuple_assignment_is_full() {
    let settings = extract("INSTALLED_APPS = ('django.contrib.auth', 'app')");
    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Complete
    );
    assert_eq!(
        settings.installed_apps.values,
        ["django.contrib.auth", "app"]
    );
}

#[test]
fn annotated_assignment_is_full() {
    let settings = extract("INSTALLED_APPS: list[str] = ['app']");
    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Complete
    );
    assert_eq!(settings.installed_apps.values, ["app"]);
}

#[test]
fn plus_equals_extends_existing_values() {
    assert_eq!(
        extract("INSTALLED_APPS = ['base']\nINSTALLED_APPS += ['extra']")
            .installed_apps
            .values,
        ["base", "extra"]
    );
}

#[test]
fn plus_chain_combines_literal_lists() {
    assert_eq!(
        extract("INSTALLED_APPS = ['a'] + ['b'] + ('c',)")
            .installed_apps
            .values,
        ["a", "b", "c"]
    );
}

#[test]
fn plus_chain_splices_known_name() {
    assert_eq!(
        extract("INSTALLED_APPS = ['a']\nINSTALLED_APPS = INSTALLED_APPS + ['b']")
            .installed_apps
            .values,
        ["a", "b"]
    );
}

#[test]
fn mutation_methods_update_values() {
    assert_eq!(
        extract(
            "INSTALLED_APPS = ['a', 'c']\n\
             INSTALLED_APPS.append('d')\n\
             INSTALLED_APPS.extend(['e'])\n\
             INSTALLED_APPS.insert(1, 'b')\n\
             INSTALLED_APPS.remove('c')",
        )
        .installed_apps
        .values,
        ["a", "b", "d", "e"]
    );
}

#[test]
fn reassignment_replaces_prior_values() {
    assert_eq!(
        extract(
            "INSTALLED_APPS = ['old']\nINSTALLED_APPS.append('ignored')\nINSTALLED_APPS = ['new']"
        )
        .installed_apps
        .values,
        ["new"]
    );
}

#[test]
fn unsupported_branch_mutation_remains_partial_when_other_branch_assigns() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'context_processors': []}}]\n\
         if FLAG:\n\
             TEMPLATES[0]['OPTIONS']['context_processors'].append('django.template.context_processors.request')\n\
         else:\n\
             TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'context_processors': []}}]",
    );

    assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
    assert!(settings.templates.backends.is_empty());
}

#[test]
fn unsupported_branch_mutation_is_order_independent() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'context_processors': []}}]\n\
         if FLAG:\n\
             TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'context_processors': []}}]\n\
         else:\n\
             TEMPLATES[0]['OPTIONS']['context_processors'].append('django.template.context_processors.request')",
    );

    assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
    assert!(settings.templates.backends.is_empty());
}

#[test]
fn non_literal_element_is_partial_and_skipped() {
    let settings = extract("INSTALLED_APPS = ['a', env('EXTRA'), 'b']");
    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert_eq!(settings.installed_apps.values, ["a", "b"]);
}

#[test]
fn unsupported_assignment_is_unsupported() {
    let settings = extract("INSTALLED_APPS = get_apps()");
    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert!(settings.installed_apps.values.is_empty());
}

#[test]
fn decidable_if_true_picks_body() {
    assert_eq!(
        extract("if True:\n    INSTALLED_APPS = ['body']\nelse:\n    INSTALLED_APPS = ['else']")
            .installed_apps
            .values,
        ["body"]
    );
}

#[test]
fn decidable_if_false_picks_else() {
    assert_eq!(
        extract("if False:\n    INSTALLED_APPS = ['body']\nelse:\n    INSTALLED_APPS = ['else']")
            .installed_apps
            .values,
        ["else"]
    );
}

#[test]
fn bool_name_condition_is_decidable() {
    assert_eq!(
        extract("DEBUG = True\nif DEBUG:\n    INSTALLED_APPS = ['debug']\nelse:\n    INSTALLED_APPS = ['prod']")
            .installed_apps
            .values,
        ["debug"]
    );
}

#[test]
fn later_assignment_replaces_unsupported_touch_uncertainty() {
    let settings =
        extract("INSTALLED_APPS = ['old']\nconfigure(INSTALLED_APPS)\nINSTALLED_APPS = ['new']");

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Complete
    );
    assert_eq!(settings.installed_apps.values, ["new"]);
}

#[test]
fn later_assignment_replaces_unresolved_star_import_uncertainty() {
    let settings = extract("from missing import *\nINSTALLED_APPS = ['local']");

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Complete
    );
    assert_eq!(settings.installed_apps.values, ["local"]);
}

#[test]
fn addition_from_partial_local_binding_stays_partial() {
    let settings = extract("if FLAG:\n    LOCAL_APPS = ['a']\nINSTALLED_APPS = LOCAL_APPS + ['b']");

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert_eq!(settings.installed_apps.values, ["a", "b"]);
}

#[test]
fn ambiguous_condition_walks_all_arms_and_marks_partial() {
    let settings = extract(
        "INSTALLED_APPS = ['base']\nif os.environ.get('X'):\n    INSTALLED_APPS.append('debug')\nelse:\n    INSTALLED_APPS.append('prod')",
    );
    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert_eq!(settings.installed_apps.values, ["base", "debug", "prod"]);
}

#[test]
fn same_value_in_ambiguous_branches_is_partial() {
    let settings = extract(
        "if FLAG:\n    INSTALLED_APPS = ['django.contrib.admin']\nelse:\n    INSTALLED_APPS = ['django.contrib.admin']",
    );

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert_eq!(settings.installed_apps.values, ["django.contrib.admin"]);
}

#[test]
fn same_relative_path_string_from_different_files_stays_partial() {
    let settings = extract_with_modules(
        "if FLAG:\n    from one.base import STATIC_ROOT\nelse:\n    from two.base import STATIC_ROOT",
        &[
            ("one.base", "STATIC_ROOT = 'static'"),
            ("two.base", "STATIC_ROOT = 'static'"),
        ],
    );

    assert_eq!(
        settings.staticfiles.static_root.extraction,
        ExtractionStatus::Partial
    );
    let values: Vec<_> = settings
        .staticfiles
        .static_root
        .values
        .iter()
        .map(Originated::value)
        .cloned()
        .collect();
    assert_eq!(
        values,
        [
            EvaluatedPath::Resolved(Utf8PathBuf::from("/project/settings/one/static")),
            EvaluatedPath::Resolved(Utf8PathBuf::from("/project/settings/two/static")),
        ]
    );
}

#[test]
fn for_loop_degrades_touched_settings_without_loop_candidates() {
    let settings =
        extract("INSTALLED_APPS = ['base']\nfor app in EXTRA_APPS:\n    INSTALLED_APPS = ['loop']");

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert_eq!(settings.installed_apps.values, ["base"]);
}

#[test]
fn for_loop_degrades_local_list_alias_without_dropping_prior_candidates() {
    let settings = extract(
        "LOCAL_APPS = ['base']\nfor app in EXTRA_APPS:\n    LOCAL_APPS = ['loop']\nINSTALLED_APPS = LOCAL_APPS",
    );

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert_eq!(settings.installed_apps.values, ["base"]);
}

#[test]
fn while_loop_degrades_touched_settings_without_loop_candidates() {
    let settings = extract("while enabled():\n    STATIC_URL = '/loop-static/'");

    assert_eq!(
        settings.staticfiles.static_url.extraction,
        ExtractionStatus::Partial
    );
    assert!(settings.staticfiles.static_url.values.is_empty());
}

#[test]
fn try_except_joins_alternative_setting_assignments() {
    let settings = extract(
        "try:\n    INSTALLED_APPS = ['try']\nexcept ImportError:\n    INSTALLED_APPS = ['except']",
    );

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert_eq!(settings.installed_apps.values, ["try", "except"]);
}

#[test]
fn try_except_handler_retains_successful_try_prefix_writes() {
    let settings = extract(
        "try:\n    INSTALLED_APPS = ['base']\n    risky()\nexcept Exception:\n    INSTALLED_APPS += ['fallback']",
    );

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert_eq!(settings.installed_apps.values, ["base", "fallback"]);
}

#[test]
fn try_except_no_exception_path_runs_else() {
    let settings = extract(
        "try:\n    INSTALLED_APPS = ['try']\nexcept Exception:\n    INSTALLED_APPS = ['except']\nelse:\n    INSTALLED_APPS += ['else']",
    );

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert_eq!(settings.installed_apps.values, ["try", "else", "except"]);
}

#[test]
fn try_except_all_paths_run_finally() {
    let settings = extract(
        "try:\n    INSTALLED_APPS = ['try']\nexcept Exception:\n    INSTALLED_APPS = ['except']\nfinally:\n    INSTALLED_APPS = ['finally']",
    );

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Complete
    );
    assert_eq!(settings.installed_apps.values, ["finally"]);
}

#[test]
fn try_except_preserves_pre_try_candidates_when_exception_may_happen_before_write() {
    let settings = extract(
        "INSTALLED_APPS = ['base']\ntry:\n    risky()\n    INSTALLED_APPS = ['try']\nexcept Exception:\n    pass",
    );

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert_eq!(settings.installed_apps.values, ["try", "base"]);
}

#[test]
fn match_joins_case_assignments() {
    let settings = extract(
        "match ENV:\n    case 'prod':\n        INSTALLED_APPS = ['prod']\n    case _:\n        INSTALLED_APPS = ['dev']",
    );

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert_eq!(settings.installed_apps.values, ["prod", "dev"]);
}

#[test]
fn match_or_pattern_with_wildcard_is_exhaustive() {
    let settings = extract("match ENV:\n    case 'prod' | _:\n        INSTALLED_APPS = ['app']");

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Complete
    );
    assert_eq!(settings.installed_apps.values, ["app"]);
}

#[test]
fn match_capture_pattern_is_irrefutable() {
    let settings = extract("match ENV:\n    case captured:\n        INSTALLED_APPS = ['app']");

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Complete
    );
    assert_eq!(settings.installed_apps.values, ["app"]);
}

#[test]
fn match_as_capture_pattern_is_irrefutable() {
    let settings = extract("match ENV:\n    case _ as captured:\n        INSTALLED_APPS = ['app']");

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Complete
    );
    assert_eq!(settings.installed_apps.values, ["app"]);
}

#[test]
fn match_capture_pattern_shadows_existing_local_binding() {
    let settings = extract(
        "from pathlib import Path\nBASE_DIR = Path(__file__).resolve().parent.parent\nmatch ENV:\n    case BASE_DIR:\n        TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [BASE_DIR / 'templates']}]",
    );

    assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
    assert_eq!(
        settings.templates.backends[0].dirs,
        [EvaluatedPath::Unknown]
    );
}

#[test]
fn duplicate_context_processor_keys_use_last_value() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'context_processors': ['project.context_processors.first'], 'context_processors': ['project.context_processors.second']}}]",
    );

    let processors = &settings.templates.backends[0].context_processors;
    assert_eq!(processors.len(), 1);
    assert_eq!(
        processors[0].value().as_str(),
        "project.context_processors.second"
    );
}

#[test]
fn invalid_overwritten_context_processor_value_does_not_mark_partial() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'context_processors': [unknown], 'context_processors': ['project.context_processors.second']}}]",
    );

    let backend = &settings.templates.backends[0];
    assert!(backend.is_fully_extracted());
    assert_eq!(
        backend.context_processors[0].value().as_str(),
        "project.context_processors.second"
    );
}

#[test]
fn duplicate_template_library_aliases_use_last_value() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {'custom': 'project.templatetags.first', 'custom': 'project.templatetags.second'}}}]",
    );

    let libraries = &settings.templates.backends[0].libraries;
    assert_eq!(libraries.len(), 1);
    assert_eq!(libraries[0].0, "custom");
    assert_eq!(libraries[0].1.as_str(), "project.templatetags.second");
}

#[test]
fn invalid_overwritten_template_library_value_does_not_mark_partial() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {'custom': unknown, 'custom': 'project.templatetags.second'}}}]",
    );

    let backend = &settings.templates.backends[0];
    assert!(backend.is_fully_extracted());
    assert_eq!(backend.libraries[0].0, "custom");
    assert_eq!(
        backend.libraries[0].1.as_str(),
        "project.templatetags.second"
    );
}

#[test]
fn invalid_overwritten_template_backend_value_does_not_mark_partial() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': unknown, 'DIRS': ['templates']}]",
    );

    let backend = &settings.templates.backends[0];
    assert!(backend.is_fully_extracted());
    assert_eq!(
        backend.dirs,
        [EvaluatedPath::Resolved(Utf8PathBuf::from(
            "/project/settings/config/templates"
        ))]
    );
}

#[test]
fn template_backend_spread_keeps_prior_known_facts_partial() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['templates'], **extra}]",
    );

    assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
    assert_eq!(
        settings.templates.backends[0].dirs,
        [EvaluatedPath::Resolved(Utf8PathBuf::from(
            "/project/settings/config/templates"
        ))]
    );
}

#[test]
fn template_options_spread_keeps_prior_known_facts_partial() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'context_processors': ['project.context_processors.first'], **extra}}]",
    );

    assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
    let processors = &settings.templates.backends[0].context_processors;
    assert_eq!(processors.len(), 1);
    assert_eq!(
        processors[0].value().as_str(),
        "project.context_processors.first"
    );
}

#[test]
fn template_library_spread_keeps_prior_known_aliases_partial() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {'custom': 'project.templatetags.first', **extra}}}]",
    );

    assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
    let libraries = &settings.templates.backends[0].libraries;
    assert_eq!(libraries.len(), 1);
    assert_eq!(libraries[0].0, "custom");
    assert_eq!(libraries[0].1.as_str(), "project.templatetags.first");
}

#[test]
fn unsupported_dict_expression_touching_known_setting_degrades_it() {
    let settings = extract("INSTALLED_APPS = ['base']\nconfigure({'apps': INSTALLED_APPS})");

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert!(settings.installed_apps.values.is_empty());
}

#[test]
fn star_import_without_setting_does_not_overwrite_existing_fact() {
    let settings = extract_with_modules(
        "INSTALLED_APPS = ['local']\nfrom paths import *",
        &[("paths", "BASE_DIR = Path(__file__).resolve().parent")],
    );
    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Complete
    );
    assert_eq!(settings.installed_apps.values, ["local"]);
}

#[test]
fn star_imported_bool_overwrites_stale_local_path_binding() {
    let settings = extract_with_modules(
        "from pathlib import Path\n\
         BASE_DIR = Path(__file__).resolve().parent.parent\n\
         from flags import *\n\
         TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [BASE_DIR / 'templates']}]",
        &[("flags", "BASE_DIR = False")],
    );

    assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
    assert_eq!(
        settings.templates.backends[0].dirs,
        [EvaluatedPath::Unknown]
    );
}

#[test]
fn star_imported_path_overwrites_stale_local_bool_binding() {
    let settings = extract_with_modules(
        "BASE_DIR = False\n\
         from paths import *\n\
         TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [BASE_DIR / 'templates']}]",
        &[("paths", "BASE_DIR = Path(__file__).resolve().parent.parent")],
    );

    assert_eq!(settings.templates.extraction, ExtractionStatus::Complete);
    assert_eq!(
        settings.templates.backends[0].dirs,
        [EvaluatedPath::Resolved(Utf8PathBuf::from(
            "/project/templates"
        ))]
    );
}

#[test]
fn cyclic_star_import_does_not_recurse_forever() {
    let settings = extract_with_modules(
        "from cycle import *",
        &[("cycle", "from cycle import *\nINSTALLED_APPS = ['local']")],
    );

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Complete
    );
    assert_eq!(settings.installed_apps.values, ["local"]);
}

#[test]
fn unresolvable_star_import_is_partial() {
    let settings = extract("from missing import *");
    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
}

#[test]
fn aliased_non_star_imported_installed_apps_can_feed_assignment() {
    let settings = extract_with_modules(
        "from base import INSTALLED_APPS as IA\nINSTALLED_APPS = IA + ['local']",
        &[("base", "INSTALLED_APPS = ['base']")],
    );

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Complete
    );
    assert_eq!(settings.installed_apps.values, ["base", "local"]);
}

#[test]
fn refused_non_star_import_falls_back_to_definition_write() {
    let mut db = TestDatabase::new();
    db.add_file("/vendor/base.py", "INSTALLED_APPS = ['base']");
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project/settings"),
        &Interpreter::Auto,
        &[Utf8PathBuf::from("/vendor")],
    );
    let project = ProjectFixture::new("/project/settings")
        .django_settings_module("config.settings")
        .search_paths(search_paths)
        .file("/project/settings/config/__init__.py", "")
        .file(
            "/project/settings/config/settings.py",
            "from base import INSTALLED_APPS",
        )
        .install(&mut db);
    let settings: ExtractedSettings = serde_json::from_value(
        serde_json::to_value(django_settings(&db, project)).expect("settings should serialize"),
    )
    .expect("settings should deserialize into the test projection");

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert!(settings.installed_apps.values.is_empty());
}

#[test]
fn imported_parse_error_marks_settings_unparseable() {
    let settings = extract_with_modules(
        "from base import INSTALLED_APPS",
        &[("base", "INSTALLED_APPS = [")],
    );

    assert_eq!(settings.parse_status, ParseStatus::Unparseable);
    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
}

#[test]
fn pathlib_named_import_does_not_affect_extraction_when_unresolved() {
    let settings = extract(
        "from pathlib import Path\n\
         BASE_DIR = Path(__file__).resolve().parent.parent\n\
         TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [BASE_DIR / 'templates']}]",
    );

    assert_eq!(settings.templates.extraction, ExtractionStatus::Complete);
    assert_eq!(
        settings.templates.backends[0].dirs,
        [EvaluatedPath::Resolved(Utf8PathBuf::from(
            "/project/settings/templates"
        ))]
    );
}

#[test]
fn template_dirs_string_list_resolves_relative_paths() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['templates']}]",
    );

    assert_eq!(settings.templates.extraction, ExtractionStatus::Complete);
    assert_eq!(
        settings.templates.backends[0].dirs,
        [EvaluatedPath::Resolved(Utf8PathBuf::from(
            "/project/settings/config/templates"
        ))]
    );
}

#[test]
fn bare_template_dirs_string_is_partial() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': 'templates'}]",
    );

    let backend = &settings.templates.backends[0];
    assert_eq!(backend.extraction, ExtractionStatus::Partial);
    assert!(backend.dirs.is_empty());
}

#[test]
fn aliased_non_star_imported_path_can_feed_template_dirs() {
    let settings = extract_with_modules(
        "from base import BASE_DIR as BD\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [BD / 'templates']}]",
        &[("base", "BASE_DIR = Path(__file__).resolve().parent.parent")],
    );

    assert_eq!(settings.templates.extraction, ExtractionStatus::Complete);
    assert_eq!(
        settings.templates.backends[0].dirs,
        [EvaluatedPath::Resolved(Utf8PathBuf::from(
            "/project/templates"
        ))]
    );
}

#[test]
fn non_star_import_chain_reuses_extracted_imported_bindings() {
    let settings = extract_with_modules(
        "from base import INSTALLED_APPS",
        &[
            ("common", "COMMON_APPS = ['common']"),
            (
                "base",
                "from common import COMMON_APPS\nINSTALLED_APPS = COMMON_APPS + ['base']",
            ),
        ],
    );

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Complete
    );
    assert_eq!(settings.installed_apps.values, ["common", "base"]);
}

#[test]
fn cyclic_non_star_import_does_not_recurse_forever() {
    let settings = extract_with_modules(
        "from cycle import INSTALLED_APPS",
        &[(
            "cycle",
            "from cycle import INSTALLED_APPS\nINSTALLED_APPS = ['local']",
        )],
    );

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Complete
    );
    assert_eq!(settings.installed_apps.values, ["local"]);
}

#[test]
fn tuple_literal_local_can_feed_installed_apps() {
    let settings = extract("LOCAL_APPS = ('a', 'b')\nINSTALLED_APPS = LOCAL_APPS");

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Complete
    );
    assert_eq!(settings.installed_apps.values, ["a", "b"]);
}

#[test]
fn local_list_unknown_write_invalidates_stale_values() {
    let settings =
        extract("LOCAL_APPS = ['stale']\nLOCAL_APPS = get_apps()\nINSTALLED_APPS = LOCAL_APPS");

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert!(settings.installed_apps.values.is_empty());
}

#[test]
fn templates_dirs_append_mutates_existing_backend() {
    let settings = extract(
        "from pathlib import Path\n\
         BASE_DIR = Path(__file__).resolve().parent.parent\n\
         TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': []}]\n\
         TEMPLATES[0]['DIRS'].append(BASE_DIR / 'templates')",
    );
    assert_eq!(
        settings.templates.backends[0].dirs,
        [EvaluatedPath::Resolved(Utf8PathBuf::from(
            "/project/settings/templates"
        ))]
    );
}

#[test]
fn templates_dirs_plus_equals_extends_existing_backend() {
    let settings = extract(
        "from pathlib import Path\n\
         BASE_DIR = Path(__file__).resolve().parent.parent\n\
         TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': []}]\n\
         TEMPLATES[0]['DIRS'] += [BASE_DIR / 'templates']",
    );
    assert_eq!(
        settings.templates.backends[0].dirs,
        [EvaluatedPath::Resolved(Utf8PathBuf::from(
            "/project/settings/templates"
        ))]
    );
}

#[test]
fn missing_backend_is_partial() {
    let settings = extract("TEMPLATES = [{'DIRS': []}]");
    assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
    assert_eq!(settings.templates.backends[0].backend, None);
    assert_eq!(
        settings.templates.backends[0].extraction,
        ExtractionStatus::Partial
    );
}

#[test]
fn non_literal_backend_is_partial() {
    let settings = extract("TEMPLATES = [{'BACKEND': backend_name}]");
    assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
    assert_eq!(settings.templates.backends[0].backend, None);
    assert_eq!(
        settings.templates.backends[0].extraction,
        ExtractionStatus::Partial
    );
}

#[test]
fn template_backend_spread_then_reset_keeps_later_key_fact() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['a'], **extra, 'DIRS': ['b']}]",
    );

    assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
    assert_eq!(settings.templates.backends[0].dirs.len(), 1);
    assert_eq!(
        settings.templates.backends[0].dirs[0],
        EvaluatedPath::Resolved(Utf8PathBuf::from("/project/settings/config/b"))
    );
}

#[test]
fn os_path_join_resolves_relative_to_base_dir() {
    let settings = extract(
        "from pathlib import Path\n\
         import os\n\
         BASE_DIR = Path(__file__).resolve().parent.parent\n\
         TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [os.path.join(BASE_DIR, 'templates')]}]",
    );
    assert_eq!(
        settings.templates.backends[0].dirs,
        [EvaluatedPath::Resolved(Utf8PathBuf::from(
            "/project/settings/templates"
        ))]
    );
}

#[test]
fn unknown_path_call_becomes_unknown_path_value() {
    let settings = extract(
        "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [dynamic_path()]}]",
    );
    assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
    assert!(matches!(
        settings.templates.backends[0].dirs[0],
        EvaluatedPath::Unknown
    ));
}

#[test]
fn ambiguous_assignment_preserves_pre_branch_possibility() {
    let settings = extract("INSTALLED_APPS = ['base']\nif FLAG:\n    INSTALLED_APPS = ['debug']");
    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert_eq!(settings.installed_apps.values, ["debug", "base"]);
}

#[test]
fn ambiguous_branch_local_alias_preserves_possible_values() {
    let settings = extract(
        "if FLAG:\n    LOCAL_APPS = ['a']\nelse:\n    LOCAL_APPS = ['b']\nINSTALLED_APPS = LOCAL_APPS",
    );

    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert_eq!(settings.installed_apps.values, ["a", "b"]);
}

#[test]
fn unsupported_assignment_then_valid_assignment_is_full() {
    let settings = extract("INSTALLED_APPS = get_apps()\nINSTALLED_APPS = ['blog']");
    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Complete
    );
    assert_eq!(settings.installed_apps.values, ["blog"]);
}

#[test]
fn unsupported_assignment_followed_by_soft_demotion_stays_unsupported() {
    let settings = extract("INSTALLED_APPS = get_apps()\nfrom missing import *");
    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert!(settings.installed_apps.values.is_empty());
}

#[test]
fn syntax_error_without_prior_settings_returns_partial_settings() {
    let settings = extract("INSTALLED_APPS = [");
    assert_eq!(
        settings.installed_apps.extraction,
        ExtractionStatus::Partial
    );
    assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
}

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_project::Db as ProjectDb;
use djls_project::*;
use djls_source::Db as _;
use djls_source::FileSystem;
use djls_source::OsFileSystem;
use djls_source::SourceFiles;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;
use serde::Deserialize;

#[derive(Deserialize)]
struct DjangoFactsGolden {
    template_dirs: Vec<String>,
    template_libraries: GoldenTemplateLibraries,
}

#[derive(Deserialize)]
struct GoldenTemplateLibraries {
    builtins: Vec<String>,
    libraries: BTreeMap<String, String>,
    symbols: Vec<GoldenTemplateSymbol>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
struct GoldenTemplateSymbol {
    kind: TemplateSymbolKind,
    name: String,
    load_name: Option<String>,
    library_module: String,
    module: String,
}

#[salsa::db]
#[derive(Clone)]
struct OsTestDatabase {
    storage: salsa::Storage<Self>,
    fs: Arc<OsFileSystem>,
    files: SourceFiles,
    project: Option<Project>,
}

impl OsTestDatabase {
    fn new() -> Self {
        Self {
            storage: salsa::Storage::default(),
            fs: Arc::new(OsFileSystem),
            files: SourceFiles::default(),
            project: None,
        }
    }
}

#[salsa::db]
impl salsa::Database for OsTestDatabase {}

#[salsa::db]
impl djls_source::Db for OsTestDatabase {
    fn files(&self) -> &SourceFiles {
        &self.files
    }

    fn file_system(&self) -> &dyn FileSystem {
        self.fs.as_ref()
    }
}

#[salsa::db]
impl ProjectDb for OsTestDatabase {
    fn project(&self) -> Option<Project> {
        self.project
    }
}

fn project_with_settings(
    db: &mut TestDatabase,
    settings_module: &str,
    files: &[(&str, &str)],
) -> Project {
    let mut fixture = ProjectFixture::new("/proj").django_settings_module(settings_module);
    for (path, source) in files {
        fixture = fixture.file(*path, *source);
    }
    fixture.install(db)
}

fn apply_project_refresh(db: &mut TestDatabase) {
    let project = db.project().expect("project should be configured");
    let refresh = RefreshData::from_query_results(
        RefreshQuery::ALL
            .iter()
            .copied()
            .map(|query| query.compute(db, project)),
    );

    apply_refresh(db, refresh);
}

#[test]
fn settings_module_file_resolves_python_module() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[("/proj/myproject/settings.py", "INSTALLED_APPS = []\n")],
    );

    let file = settings_module_file(&db, project).expect("settings module should resolve");

    assert_eq!(file.path(&db), Utf8Path::new("/proj/myproject/settings.py"));
}

#[test]
fn settings_module_file_returns_none_for_missing_module() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(&mut db, "myproject.settings", &[]);

    assert!(settings_module_file(&db, project).is_none());
}

#[test]
fn django_settings_resolves_relative_star_imports() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.prod",
        &[
            (
                "/proj/myproject/base.py",
                "INSTALLED_APPS = ['django.contrib.auth']\n",
            ),
            (
                "/proj/myproject/prod.py",
                "from .base import *\nINSTALLED_APPS += ['blog']\n",
            ),
        ],
    );

    let settings = django_settings(&db, project);

    assert_eq!(settings.installed_apps.knowledge, StaticKnowledge::Known);
    assert_eq!(
        settings.installed_apps.values,
        vec!["django.contrib.auth".to_string(), "blog".to_string()]
    );
}

#[test]
fn django_settings_recovers_from_star_import_cycle() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[(
            "/proj/myproject/settings.py",
            "from .settings import *\nINSTALLED_APPS = ['blog']\n",
        )],
    );

    let settings = django_settings(&db, project);

    assert_eq!(settings.installed_apps.knowledge, StaticKnowledge::Known);
    assert_eq!(settings.installed_apps.values, vec!["blog".to_string()]);
}

#[test]
fn template_dirs_include_dirs_entries_before_app_dirs() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/templates/base.html", "base"),
            ("/proj/blog/__init__.py", ""),
            ("/proj/blog/templates/blog/detail.html", "detail"),
            (
                "/proj/myproject/settings.py",
                "from pathlib import Path\nBASE_DIR = Path(__file__).resolve().parent.parent\nINSTALLED_APPS = ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [BASE_DIR / 'templates'], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let (dirs, knowledge) = template_dirs(&db, project).clone();

    assert_eq!(knowledge, StaticKnowledge::Known);
    assert_eq!(
        dirs,
        vec![
            Utf8PathBuf::from("/proj/templates"),
            Utf8PathBuf::from("/proj/blog/templates"),
        ]
    );
}

#[test]
fn template_dirs_resolve_app_config_entries() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/blog/apps.py", ""),
            ("/proj/blog/templates/blog/detail.html", "detail"),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['blog.apps.BlogConfig']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let (dirs, knowledge) = template_dirs(&db, project).clone();

    assert_eq!(knowledge, StaticKnowledge::Known);
    assert_eq!(dirs, vec![Utf8PathBuf::from("/proj/blog/templates")]);
}

#[test]
fn template_dirs_resolve_apps_from_site_packages_search_path() {
    let mut db = TestDatabase::new();
    db.add_file("/site/pkg/__init__.py", "");
    db.add_file("/site/pkg/templates/pkg/index.html", "index");
    db.add_file(
        "/proj/myproject/settings.py",
        "INSTALLED_APPS = ['pkg']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
    );
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/proj"),
        &Interpreter::Auto,
        &["/site".to_string()],
    );
    search_paths.register_roots(&db);
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .search_paths(search_paths)
        .install(&mut db);

    let (dirs, knowledge) = template_dirs(&db, project).clone();

    assert_eq!(knowledge, StaticKnowledge::Known);
    assert_eq!(dirs, vec![Utf8PathBuf::from("/site/pkg/templates")]);
}

#[test]
fn template_dirs_demote_unresolved_app_to_partial() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[(
            "/proj/myproject/settings.py",
            "INSTALLED_APPS = ['missing']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
        )],
    );

    let (dirs, knowledge) = template_dirs(&db, project).clone();

    assert!(dirs.is_empty());
    assert_eq!(knowledge, StaticKnowledge::Partial);
}

#[test]
fn template_libraries_discover_app_templatetags_and_builtins() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/django/template/defaulttags.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef default_tag():\n    pass\n",
            ),
            (
                "/proj/django/template/defaultfilters.py",
                "from django import template\nregister = template.Library()\n@register.filter\ndef default_filter(value):\n    return value\n",
            ),
            (
                "/proj/django/template/loader_tags.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef loader_tag():\n    pass\n",
            ),
            ("/proj/blog/templatetags/__init__.py", ""),
            (
                "/proj/blog/templatetags/custom.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef hello():\n    pass\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);

    assert_eq!(libraries.knowledge, StaticKnowledge::Known);
    let custom = libraries
        .loadable_library_str("custom")
        .expect("custom library should be discovered");
    assert_eq!(custom.module().as_str(), "blog.templatetags.custom");
    assert!(custom.symbols.iter().any(|symbol| symbol.name() == "hello"));
    assert_eq!(
        libraries
            .builtin_modules()
            .map(PyModuleName::as_str)
            .collect::<Vec<_>>(),
        vec![
            "django.template.defaulttags",
            "django.template.defaultfilters",
            "django.template.loader_tags",
        ]
    );
}

#[test]
fn template_libraries_include_empty_registered_modules() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/django/template/defaulttags.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef default_tag():\n    pass\n",
            ),
            (
                "/proj/django/template/defaultfilters.py",
                "from django import template\nregister = template.Library()\n@register.filter\ndef default_filter(value):\n    return value\n",
            ),
            (
                "/proj/django/template/loader_tags.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef loader_tag():\n    pass\n",
            ),
            ("/proj/blog/templatetags/__init__.py", ""),
            (
                "/proj/blog/templatetags/empty.py",
                "from django import template\nregister = template.Library()\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);

    let empty = libraries.loadable_library_str("empty").unwrap();
    assert_eq!(empty.module().as_str(), "blog.templatetags.empty");
    assert!(empty.symbols.is_empty());
}

#[test]
fn inactive_template_libraries_collect_uninstalled_templatetags() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/myapp/__init__.py", ""),
            ("/proj/myapp/templatetags/__init__.py", ""),
            (
                "/proj/myapp/templatetags/myapp_tags.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef active_tag():\n    pass\n",
            ),
            ("/proj/crispy/__init__.py", ""),
            ("/proj/crispy/templatetags/__init__.py", ""),
            (
                "/proj/crispy/templatetags/crispy.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef crispy_tag():\n    pass\n@register.filter\ndef crispy_filter(value):\n    return value\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['myapp']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let inactive = inactive_template_libraries(&db, project);

    let crispy = LibraryName::parse("crispy").unwrap();
    let candidates = inactive.library_candidates(&crispy);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].name.as_str(), "crispy");
    assert_eq!(candidates[0].app.as_str(), "crispy");
    assert_eq!(candidates[0].module.as_str(), "crispy.templatetags.crispy");
    assert_eq!(candidates[0].tags, vec!["crispy_tag"]);
    assert_eq!(candidates[0].filters, vec!["crispy_filter"]);
    assert!(
        inactive
            .library_candidates(&LibraryName::parse("myapp_tags").unwrap())
            .is_empty(),
        "installed app libraries must be subtracted from inactive candidates"
    );
}

#[test]
fn inactive_template_libraries_rerun_after_search_root_revision_bump() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/myapp/__init__.py", ""),
            ("/proj/myapp/templatetags/__init__.py", ""),
            (
                "/proj/myapp/templatetags/myapp_tags.py",
                "from django import template\nregister = template.Library()\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['myapp']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    assert!(
        inactive_template_libraries(&db, project)
            .library_candidates(&LibraryName::parse("crispy").unwrap())
            .is_empty()
    );

    db.add_file("/proj/crispy/__init__.py", "");
    db.add_file("/proj/crispy/templatetags/__init__.py", "");
    db.add_file(
        "/proj/crispy/templatetags/crispy.py",
        "from django import template\nregister = template.Library()\n@register.simple_tag\ndef crispy_tag():\n    pass\n",
    );
    let root = db
        .files()
        .expect_root(&db, Utf8Path::new("/proj/crispy/templatetags/crispy.py"));
    db.bump_file_root_revision(root);

    let inactive = inactive_template_libraries(&db, project);

    assert_eq!(
        inactive
            .library_candidates(&LibraryName::parse("crispy").unwrap())
            .len(),
        1
    );
}

#[test]
fn project_refresh_updates_inactive_template_library_symbols() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/myapp/__init__.py", ""),
            ("/proj/myapp/templatetags/__init__.py", ""),
            (
                "/proj/myapp/templatetags/myapp_tags.py",
                "from django import template\nregister = template.Library()\n",
            ),
            ("/proj/crispy/__init__.py", ""),
            ("/proj/crispy/templatetags/__init__.py", ""),
            (
                "/proj/crispy/templatetags/crispy.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef old_tag():\n    pass\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['myapp']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    assert!(
        inactive_template_libraries(&db, project)
            .tag_candidates("new_tag")
            .is_empty()
    );

    db.add_file(
        "/proj/crispy/templatetags/crispy.py",
        "from django import template\nregister = template.Library()\n@register.simple_tag\ndef new_tag():\n    pass\n",
    );
    apply_project_refresh(&mut db);

    assert_eq!(
        inactive_template_libraries(&db, project)
            .tag_candidates("new_tag")
            .len(),
        1
    );
}

#[test]
fn template_libraries_demote_unresolved_app_to_partial() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[(
            "/proj/myproject/settings.py",
            "INSTALLED_APPS = ['missing']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
        )],
    );

    let libraries = template_libraries(&db, project);

    assert_eq!(libraries.knowledge, StaticKnowledge::Partial);
}

#[test]
fn template_libraries_include_options_libraries_and_builtins() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/django/template/defaulttags.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef default_tag():\n    pass\n",
            ),
            (
                "/proj/django/template/defaultfilters.py",
                "from django import template\nregister = template.Library()\n@register.filter\ndef default_filter(value):\n    return value\n",
            ),
            (
                "/proj/django/template/loader_tags.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef loader_tag():\n    pass\n",
            ),
            (
                "/proj/custom_tags.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef configured():\n    pass\n",
            ),
            (
                "/proj/custom_builtin.py",
                "from django import template\nregister = template.Library()\n@register.filter\ndef configured_filter(value):\n    return value\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'custom_tags'}, 'builtins': ['custom_builtin']}}]\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);

    let custom = libraries.loadable_library_str("custom").unwrap();
    assert_eq!(custom.module().as_str(), "custom_tags");
    assert!(
        custom
            .symbols
            .iter()
            .any(|symbol| symbol.name() == "configured")
    );
    assert!(
        libraries
            .builtin_libraries()
            .flat_map(|library| &library.symbols)
            .any(|symbol| symbol.name() == "configured_filter")
    );
}

#[test]
fn template_libraries_keep_configured_libraries_when_installed_apps_unknown() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/django/template/defaulttags.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef default_tag():\n    pass\n",
            ),
            (
                "/proj/django/template/defaultfilters.py",
                "from django import template\nregister = template.Library()\n@register.filter\ndef default_filter(value):\n    return value\n",
            ),
            (
                "/proj/django/template/loader_tags.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef loader_tag():\n    pass\n",
            ),
            (
                "/proj/project_tags.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef configured():\n    pass\n",
            ),
            (
                "/proj/myproject/settings.py",
                "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'project_tags'}}}]\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);

    assert_eq!(libraries.knowledge, StaticKnowledge::Partial);
    let custom = libraries.loadable_library_str("custom").unwrap();
    assert_eq!(custom.module().as_str(), "project_tags");
    assert!(
        custom
            .symbols
            .iter()
            .any(|symbol| symbol.name() == "configured")
    );
}

#[test]
fn template_libraries_options_override_app_library_load_name() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/django/template/defaulttags.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef default_tag():\n    pass\n",
            ),
            (
                "/proj/django/template/defaultfilters.py",
                "from django import template\nregister = template.Library()\n@register.filter\ndef default_filter(value):\n    return value\n",
            ),
            (
                "/proj/django/template/loader_tags.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef loader_tag():\n    pass\n",
            ),
            ("/proj/blog/templatetags/__init__.py", ""),
            (
                "/proj/blog/templatetags/custom.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef old_tag():\n    pass\n",
            ),
            (
                "/proj/project_tags.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef new_tag():\n    pass\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True, 'OPTIONS': {'libraries': {'custom': 'project_tags'}}}]\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);

    let custom = libraries.loadable_library_str("custom").unwrap();
    assert_eq!(custom.module().as_str(), "project_tags");
    assert!(
        custom
            .symbols
            .iter()
            .any(|symbol| symbol.name() == "new_tag")
    );
    assert!(
        !custom
            .symbols
            .iter()
            .any(|symbol| symbol.name() == "old_tag")
    );
}

#[test]
#[ignore = "requires the e2e Django virtualenv with Django installed"]
fn django_facts_golden_template_dirs_match() {
    let Ok(_venv) = std::env::var("VIRTUAL_ENV") else {
        eprintln!("skipping golden comparison because VIRTUAL_ENV is not set");
        return;
    };
    let workspace = Utf8PathBuf::from_path_buf(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap(),
    )
    .unwrap();
    let project_root = workspace.join("tests/project");
    let golden_path = workspace.join("tests/fixtures/django-facts/django-5.2.json");
    let golden: DjangoFactsGolden =
        serde_json::from_str(&std::fs::read_to_string(golden_path.as_std_path()).unwrap()).unwrap();

    let mut db = OsTestDatabase::new();
    let settings = djls_conf::Settings::new(project_root.as_path(), None).unwrap();
    let project = Project::bootstrap(&db, project_root.as_path(), &settings);
    db.project = Some(project);

    let site_packages = project
        .search_paths(&db)
        .iter()
        .find_map(|search_path| {
            let path = search_path.path();
            path.components()
                .any(|component| matches!(component.as_str(), "site-packages" | "dist-packages"))
                .then_some(path)
        })
        .expect("e2e venv should provide site-packages");
    let expected: Vec<_> = golden
        .template_dirs
        .into_iter()
        .map(|path| {
            path.replace("${PROJECT}", project_root.as_str())
                .replace("${SITE_PACKAGES}", site_packages.as_str())
        })
        .collect();
    let actual: Vec<_> = template_dirs(&db, project)
        .0
        .iter()
        .map(ToString::to_string)
        .collect();

    assert_eq!(actual, expected);
}

#[test]
#[ignore = "requires the e2e Django virtualenv with Django installed"]
fn django_facts_golden_template_libraries_match() {
    let Ok(_venv) = std::env::var("VIRTUAL_ENV") else {
        eprintln!("skipping golden comparison because VIRTUAL_ENV is not set");
        return;
    };
    let workspace = Utf8PathBuf::from_path_buf(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap(),
    )
    .unwrap();
    let project_root = workspace.join("tests/project");
    let golden_path = workspace.join("tests/fixtures/django-facts/django-5.2.json");
    let golden: DjangoFactsGolden =
        serde_json::from_str(&std::fs::read_to_string(golden_path.as_std_path()).unwrap()).unwrap();

    let mut db = OsTestDatabase::new();
    let settings = djls_conf::Settings::new(project_root.as_path(), None).unwrap();
    let project = Project::bootstrap(&db, project_root.as_path(), &settings);
    db.project = Some(project);

    let libraries = template_libraries(&db, project);
    let actual_builtins: Vec<_> = libraries
        .builtin_modules()
        .map(|module| module.as_str().to_string())
        .collect();
    assert_eq!(actual_builtins, golden.template_libraries.builtins);

    let actual_libraries: BTreeMap<_, _> = libraries
        .loadable_libraries()
        .map(|(name, library)| {
            (
                name.as_str().to_string(),
                library.module().as_str().to_string(),
            )
        })
        .collect();
    assert_eq!(actual_libraries, golden.template_libraries.libraries);

    let mut actual_symbols = comparable_symbols(libraries);
    let mut expected_symbols = golden.template_libraries.symbols;
    actual_symbols.sort();
    expected_symbols.sort();
    assert_eq!(actual_symbols, expected_symbols);
}

fn comparable_symbols(libraries: &TemplateLibraries) -> Vec<GoldenTemplateSymbol> {
    let mut symbols = Vec::new();

    for library in libraries.builtin_libraries() {
        for symbol in &library.symbols {
            symbols.push(GoldenTemplateSymbol {
                kind: symbol.kind,
                name: symbol.name().to_string(),
                load_name: None,
                library_module: library.module().as_str().to_string(),
                module: library.module().as_str().to_string(),
            });
        }
    }

    for (load_name, library) in libraries.loadable_libraries() {
        for symbol in &library.symbols {
            symbols.push(GoldenTemplateSymbol {
                kind: symbol.kind,
                name: symbol.name().to_string(),
                load_name: Some(load_name.as_str().to_string()),
                library_module: library.module().as_str().to_string(),
                module: library.module().as_str().to_string(),
            });
        }
    }

    symbols
}

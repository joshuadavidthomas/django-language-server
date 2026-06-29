use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_project::Db as ProjectDb;
use djls_project::testing::compute_refresh;
use djls_project::*;
use djls_source::Db as _;
use djls_source::FileSystem;
use djls_source::InMemoryFileSystem;
use djls_source::OsFileSystem;
use djls_source::SourceFiles;
use djls_source::WalkEntry;
use djls_source::WalkOptions;
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

fn inactive_library_candidates<'a>(
    libraries: &'a TemplateLibraries,
    load_name: &str,
) -> Vec<&'a TemplateLibrary> {
    libraries
        .records()
        .filter(|library| {
            library.inactive_app().is_some()
                && library
                    .load_name()
                    .is_some_and(|name| name.as_str() == load_name)
        })
        .collect()
}

fn inactive_tag_candidates<'a>(
    libraries: &'a TemplateLibraries,
    tag: &str,
) -> Vec<&'a TemplateLibrary> {
    inactive_symbol_candidates(libraries, tag, TemplateSymbolKind::Tag)
}

fn inactive_symbol_candidates<'a>(
    libraries: &'a TemplateLibraries,
    symbol_name: &str,
    kind: TemplateSymbolKind,
) -> Vec<&'a TemplateLibrary> {
    libraries
        .records()
        .filter(|library| library.inactive_app().is_some())
        .filter(|library| {
            library
                .symbols()
                .iter()
                .any(|symbol| symbol.kind == kind && symbol.name() == symbol_name)
        })
        .collect()
}

#[salsa::db]
#[derive(Clone)]
struct OsTestDatabase {
    storage: salsa::Storage<Self>,
    fs: Arc<dyn FileSystem>,
    files: SourceFiles,
    project: Option<Project>,
}

impl OsTestDatabase {
    fn new() -> Self {
        Self::with_file_system(Arc::new(OsFileSystem))
    }

    fn with_file_system(fs: Arc<dyn FileSystem>) -> Self {
        Self {
            storage: salsa::Storage::default(),
            fs,
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

struct FailingReadFileSystem {
    inner: InMemoryFileSystem,
    unreadable: Utf8PathBuf,
}

impl FileSystem for FailingReadFileSystem {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        if path == self.unreadable.as_path() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "test file is unreadable",
            ));
        }

        self.inner.read_to_string(path)
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        self.inner.exists(path)
    }

    fn is_file(&self, path: &Utf8Path) -> bool {
        self.inner.is_file(path)
    }

    fn is_dir(&self, path: &Utf8Path) -> bool {
        self.inner.is_dir(path)
    }

    fn walk_entries(&self, root: &Utf8Path, options: &WalkOptions) -> io::Result<Vec<WalkEntry>> {
        self.inner.walk_entries(root, options)
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
    let refresh = compute_refresh(db, project);
    apply_refresh(db, refresh);
}

#[test]
fn project_refresh_enumerates_settings_star_import_chain() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/myproject/settings.py",
                "from .base import *\nfrom .feature import *\n",
            ),
            (
                "/proj/myproject/base.py",
                "from .common import *\nINSTALLED_APPS = []\n",
            ),
            ("/proj/myproject/feature.py", "from .common import *\n"),
            (
                "/proj/myproject/common.py",
                "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False}]\n",
            ),
        ],
    );

    let refresh = compute_refresh(&db, project);

    let expected = [
        Utf8PathBuf::from("/proj/myproject/base.py"),
        Utf8PathBuf::from("/proj/myproject/common.py"),
        Utf8PathBuf::from("/proj/myproject/feature.py"),
        Utf8PathBuf::from("/proj/myproject/settings.py"),
    ];
    assert_eq!(refresh.file_paths(), expected.as_slice());
}

#[test]
fn project_refresh_includes_deduped_unreadable_settings_source() {
    let unreadable = Utf8PathBuf::from("/proj/myproject/unreadable.py");
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(
        Utf8PathBuf::from("/proj/myproject/settings.py"),
        "from .base import *\nfrom .unreadable import *\nfrom .base import *\n".to_string(),
    );
    fs.add_file(
        Utf8PathBuf::from("/proj/myproject/base.py"),
        "INSTALLED_APPS = []\n".to_string(),
    );
    fs.add_file(unreadable.clone(), "TEMPLATES = []\n".to_string());

    let mut db = OsTestDatabase::with_file_system(Arc::new(FailingReadFileSystem {
        inner: fs,
        unreadable: unreadable.clone(),
    }));
    let root = Utf8PathBuf::from("/proj");
    let interpreter = Interpreter::Auto;
    let pythonpath = Vec::new();
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        root.as_path(),
        &interpreter,
        &pythonpath,
    );
    search_paths.register_roots(&db);
    let tag_specs = djls_conf::Settings::default().tagspecs().clone();
    let project = Project::new(
        &db,
        root,
        search_paths,
        interpreter,
        Some("myproject.settings".to_string()),
        pythonpath,
        Vec::new(),
        tag_specs,
    );
    db.project = Some(project);

    let settings_sources = refresh_tasks()
        .iter()
        .copied()
        .find(|task| task.descriptor().message == "Scanning settings")
        .expect("settings sources refresh task should exist")
        .run(&db, project);
    assert_eq!(settings_sources.count(), 3);

    let refresh = compute_refresh(&db, project);

    let expected = [
        Utf8PathBuf::from("/proj/myproject/base.py"),
        Utf8PathBuf::from("/proj/myproject/settings.py"),
        unreadable,
    ];
    assert_eq!(refresh.file_paths(), expected.as_slice());
}

#[test]
fn template_dirs_resolve_settings_module_file() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[(
            "/proj/myproject/settings.py",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False}]\n",
        )],
    );

    let dirs = template_resolution(&db, project)
        .known_template_dirs(&db)
        .expect("template dirs should be known");

    assert_eq!(dirs, vec![Utf8PathBuf::from("/proj/templates")]);
}

#[test]
fn template_dirs_return_unknown_for_missing_settings_module() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(&mut db, "myproject.settings", &[]);

    let dirs = template_resolution(&db, project).known_template_dirs(&db);

    assert!(dirs.is_none());
}

#[test]
fn template_dirs_resolve_relative_star_imports() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.prod",
        &[
            ("/proj/django/contrib/auth/__init__.py", ""),
            (
                "/proj/django/contrib/auth/templates/auth/index.html",
                "auth",
            ),
            ("/proj/blog/__init__.py", ""),
            ("/proj/blog/templates/blog/detail.html", "detail"),
            (
                "/proj/myproject/base.py",
                "INSTALLED_APPS = ['django.contrib.auth']\n",
            ),
            (
                "/proj/myproject/prod.py",
                "from .base import *\nINSTALLED_APPS += ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let dirs = template_resolution(&db, project)
        .known_template_dirs(&db)
        .expect("template dirs should be known");

    assert_eq!(
        dirs,
        vec![
            Utf8PathBuf::from("/proj/django/contrib/auth/templates"),
            Utf8PathBuf::from("/proj/blog/templates"),
        ]
    );
}

#[test]
fn template_dirs_resolve_relative_star_imports_from_package_module() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/django/contrib/auth/__init__.py", ""),
            (
                "/proj/django/contrib/auth/templates/auth/index.html",
                "auth",
            ),
            ("/proj/blog/__init__.py", ""),
            ("/proj/blog/templates/blog/detail.html", "detail"),
            (
                "/proj/myproject/settings/base.py",
                "INSTALLED_APPS = ['django.contrib.auth']\n",
            ),
            (
                "/proj/myproject/settings/__init__.py",
                "from .base import *\nINSTALLED_APPS += ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let dirs = template_resolution(&db, project)
        .known_template_dirs(&db)
        .expect("template dirs should be known");

    assert_eq!(
        dirs,
        vec![
            Utf8PathBuf::from("/proj/django/contrib/auth/templates"),
            Utf8PathBuf::from("/proj/blog/templates"),
        ]
    );
}

#[test]
fn template_dirs_recover_from_star_import_cycle() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/blog/__init__.py", ""),
            ("/proj/blog/templates/blog/detail.html", "detail"),
            (
                "/proj/myproject/settings.py",
                "from .settings import *\nINSTALLED_APPS = ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let dirs = template_resolution(&db, project)
        .known_template_dirs(&db)
        .expect("template dirs should be known");

    assert_eq!(dirs, vec![Utf8PathBuf::from("/proj/blog/templates")]);
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

    let dirs = template_resolution(&db, project)
        .known_template_dirs(&db)
        .expect("template dirs should be known");

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

    let dirs = template_resolution(&db, project)
        .known_template_dirs(&db)
        .expect("template dirs should be known");

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

    let dirs = template_resolution(&db, project)
        .known_template_dirs(&db)
        .expect("template dirs should be known");

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

    let dirs = template_resolution(&db, project).known_template_dirs(&db);

    assert!(dirs.is_none());
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

    assert!(libraries.inventory_is_complete());
    let custom = libraries
        .loadable_library_str("custom")
        .expect("custom library should be discovered");
    assert_eq!(custom.module_path_str(), "blog.templatetags.custom");
    assert!(
        custom
            .symbols()
            .iter()
            .any(|symbol| symbol.name() == "hello")
    );
    assert_eq!(
        libraries
            .builtin_modules()
            .map(PythonModulePath::as_str)
            .collect::<Vec<_>>(),
        vec![
            "django.template.defaulttags",
            "django.template.defaultfilters",
            "django.template.loader_tags",
        ]
    );
}

#[test]
fn template_libraries_discover_namespace_package_templatetags() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/nsapp/templatetags/__init__.py", ""),
            (
                "/proj/nsapp/templatetags/custom.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef hello():\n    pass\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['nsapp']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);

    let custom = libraries
        .loadable_library_str("custom")
        .expect("namespace package templatetag should be discovered");
    assert_eq!(custom.module_path_str(), "nsapp.templatetags.custom");
    assert!(
        custom
            .symbols()
            .iter()
            .any(|symbol| symbol.name() == "hello")
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
    assert_eq!(empty.module_path_str(), "blog.templatetags.empty");
    assert!(empty.symbols().is_empty());
}

#[test]
fn template_libraries_collect_inactive_uninstalled_templatetags() {
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

    let libraries = template_libraries(&db, project);

    let candidates = inactive_library_candidates(libraries, "crispy");
    assert_eq!(candidates.len(), 1);
    let candidate = candidates[0];
    assert_eq!(candidate.load_name().unwrap().as_str(), "crispy");
    assert_eq!(candidate.inactive_app().unwrap().as_str(), "crispy");
    assert_eq!(candidate.module_path_str(), "crispy.templatetags.crispy");
    assert_eq!(
        candidate
            .tags()
            .map(|symbol| symbol.name().to_string())
            .collect::<Vec<_>>(),
        vec!["crispy_tag"]
    );
    assert_eq!(
        candidate
            .filters()
            .map(|symbol| symbol.name().to_string())
            .collect::<Vec<_>>(),
        vec!["crispy_filter"]
    );
    assert!(
        inactive_library_candidates(libraries, "myapp_tags").is_empty(),
        "installed app libraries must be subtracted from inactive candidates"
    );
}

#[test]
fn template_libraries_inactive_candidates_rerun_after_search_root_revision_bump() {
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

    assert!(inactive_library_candidates(template_libraries(&db, project), "crispy").is_empty());

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

    let libraries = template_libraries(&db, project);

    assert_eq!(inactive_library_candidates(libraries, "crispy").len(), 1);
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

    assert!(inactive_tag_candidates(template_libraries(&db, project), "new_tag").is_empty());

    db.add_file(
        "/proj/crispy/templatetags/crispy.py",
        "from django import template\nregister = template.Library()\n@register.simple_tag\ndef new_tag():\n    pass\n",
    );
    apply_project_refresh(&mut db);

    assert_eq!(
        inactive_tag_candidates(template_libraries(&db, project), "new_tag").len(),
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

    assert!(libraries.inventory_may_omit_loaded_symbols());
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
    assert_eq!(custom.module_path_str(), "custom_tags");
    assert!(
        custom
            .symbols()
            .iter()
            .any(|symbol| symbol.name() == "configured")
    );
    assert!(
        libraries
            .installed_symbol_candidates(TemplateSymbolKind::Filter)
            .iter()
            .any(|candidate| {
                candidate.symbol.name() == "configured_filter"
                    && matches!(
                        &candidate.origin,
                        InstalledSymbolOrigin::Builtin { module, .. }
                            if module.as_str() == "custom_builtin"
                    )
            })
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

    assert!(libraries.inventory_may_omit_loaded_symbols());
    let custom = libraries.loadable_library_str("custom").unwrap();
    assert_eq!(custom.module_path_str(), "project_tags");
    assert!(
        custom
            .symbols()
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
    assert_eq!(custom.module_path_str(), "project_tags");
    assert!(
        custom
            .symbols()
            .iter()
            .any(|symbol| symbol.name() == "new_tag")
    );
    assert!(
        !custom
            .symbols()
            .iter()
            .any(|symbol| symbol.name() == "old_tag")
    );
    assert!(
        inactive_library_candidates(libraries, "custom").is_empty(),
        "a configured alias can shadow an installed app library without making that app inactive"
    );
    assert!(inactive_tag_candidates(libraries, "old_tag").is_empty());
}

#[test]
fn template_libraries_keep_invalid_configured_alias_as_unresolved_record() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[(
            "/proj/myproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'broken': 'bad-module'}}}]\n",
        )],
    );

    let libraries = template_libraries(&db, project);

    assert!(libraries.inventory_may_omit_loaded_symbols());
    let broken = libraries.loadable_library_str("broken").unwrap();
    assert_eq!(broken.load_name().unwrap().as_str(), "broken");
    assert_eq!(broken.module_path_str(), "bad-module");
    assert!(broken.module_path().is_none());
    assert!(matches!(
        broken.resolution(),
        TemplateLibraryResolution::Unresolved(
            TemplateLibraryResolutionError::InvalidModulePath(path)
        ) if path == "bad-module"
    ));
    assert!(!broken.defines_library());
    assert!(broken.symbols().is_empty());
}

#[test]
fn template_libraries_keep_configured_non_library_module_as_unresolved_record() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/not_a_library.py", "VALUE = 1\n"),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'not_a_library'}}}]\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);

    let custom = libraries.loadable_library_str("custom").unwrap();
    assert_eq!(custom.module_path_str(), "not_a_library");
    assert!(matches!(
        custom.resolution(),
        TemplateLibraryResolution::Unresolved(TemplateLibraryResolutionError::NotATemplateLibrary(
            _
        ))
    ));
    assert!(!custom.defines_library());
    assert!(custom.symbols().is_empty());
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
    let actual: Vec<_> = template_resolution(&db, project)
        .known_template_dirs(&db)
        .expect("template dirs should be known")
        .into_iter()
        .map(|path| path.to_string())
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
        .completion_library_names()
        .into_iter()
        .filter_map(|name| {
            let library = libraries.loadable_library(&name)?;
            Some((
                name.as_str().to_string(),
                library.module_path_str().to_string(),
            ))
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

    for library in libraries
        .records()
        .filter(|library| library.inactive_app().is_none())
    {
        let load_name = library.load_name().map(|name| name.as_str().to_string());

        for symbol in library.symbols() {
            symbols.push(GoldenTemplateSymbol {
                kind: symbol.kind,
                name: symbol.name().to_string(),
                load_name: load_name.clone(),
                library_module: library.module_path_str().to_string(),
                module: library.module_path_str().to_string(),
            });
        }
    }

    symbols
}

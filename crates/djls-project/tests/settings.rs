use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_project::testing::compute_django_discovery;
use djls_project::testing::django_settings;
use djls_project::*;
use djls_source::Db as _;
use djls_source::FileSystem;
use djls_source::InMemoryFileSystem;
use djls_source::WalkEntry;
use djls_source::WalkOptions;
use djls_testing::OsTestDatabase;
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

fn library_name(name: &str) -> LibraryName {
    LibraryName::parse(name).unwrap()
}

fn active_builtin_modules(libraries: &TemplateLibraries) -> Vec<String> {
    libraries
        .active_libraries()
        .filter(|&library| library.load_name().is_none())
        .map(|library| library.module_name().as_str().to_string())
        .collect()
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

    fn case_sensitivity(&self) -> djls_source::CaseSensitivity {
        self.inner.case_sensitivity()
    }

    fn path_exists_case_sensitive(&self, path: &Utf8Path, prefix: &Utf8Path) -> bool {
        self.inner.path_exists_case_sensitive(path, prefix)
    }

    fn walk_entries(&self, root: &Utf8Path, options: &WalkOptions) -> io::Result<Vec<WalkEntry>> {
        self.inner.walk_entries(root, options)
    }
}

struct FailingWalkFileSystem {
    inner: InMemoryFileSystem,
    failing_root: Utf8PathBuf,
}

impl FileSystem for FailingWalkFileSystem {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
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

    fn case_sensitivity(&self) -> djls_source::CaseSensitivity {
        self.inner.case_sensitivity()
    }

    fn path_exists_case_sensitive(&self, path: &Utf8Path, prefix: &Utf8Path) -> bool {
        self.inner.path_exists_case_sensitive(path, prefix)
    }

    fn walk_entries(&self, root: &Utf8Path, options: &WalkOptions) -> io::Result<Vec<WalkEntry>> {
        if root == self.failing_root.as_path() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "test root is unwalkable",
            ));
        }

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

fn apply_project_discovery(db: &mut TestDatabase) {
    let project = db.project().expect("project should be configured");
    let discovery = compute_django_discovery(db, project);
    apply_django_discovery(db, discovery);
}

#[test]
fn django_discovery_enumerates_settings_star_import_chain() {
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

    let discovery = compute_django_discovery(&db, project);

    let expected = [
        Utf8PathBuf::from("/proj/myproject/base.py"),
        Utf8PathBuf::from("/proj/myproject/common.py"),
        Utf8PathBuf::from("/proj/myproject/feature.py"),
        Utf8PathBuf::from("/proj/myproject/settings.py"),
    ];
    assert_eq!(discovery.file_paths(), expected.as_slice());
}

#[test]
fn settings_sources_includes_semantically_reached_imports_and_excludes_unreachable_imports() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/myproject/settings.py",
                "from .flags import DEBUG\nif DEBUG:\n    from .unreachable import *\nelse:\n    from .base import *\n",
            ),
            ("/proj/myproject/flags.py", "DEBUG = False\n"),
            ("/proj/myproject/base.py", "INSTALLED_APPS = []\n"),
            ("/proj/myproject/unreachable.py", "INSTALLED_APPS = [\n"),
        ],
    );

    let discovery = compute_django_discovery(&db, project);

    assert_eq!(
        discovery.file_paths(),
        [
            Utf8PathBuf::from("/proj/myproject/base.py"),
            Utf8PathBuf::from("/proj/myproject/flags.py"),
            Utf8PathBuf::from("/proj/myproject/settings.py"),
        ]
    );
}

#[test]
fn settings_sources_excludes_import_guarded_by_imported_false_flag() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/myproject/settings.py",
                "from .flags import DEBUG\nif DEBUG:\n    from .broken import *\nelse:\n    INSTALLED_APPS = ['local']\n",
            ),
            ("/proj/myproject/flags.py", "DEBUG = False\n"),
            ("/proj/myproject/broken.py", "INSTALLED_APPS = [\n"),
        ],
    );

    let discovery = compute_django_discovery(&db, project);

    assert_eq!(
        discovery.file_paths(),
        [
            Utf8PathBuf::from("/proj/myproject/flags.py"),
            Utf8PathBuf::from("/proj/myproject/settings.py"),
        ]
    );
}

#[test]
fn settings_sources_includes_import_after_unsupported_guard_touch() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/myproject/settings.py",
                "DEBUG = False\nmaybe_enable(DEBUG)\nif DEBUG:\n    from .broken import *\nelse:\n    INSTALLED_APPS = ['local']\n",
            ),
            ("/proj/myproject/broken.py", "INSTALLED_APPS = [\n"),
        ],
    );

    let discovery = compute_django_discovery(&db, project);

    assert_eq!(
        discovery.file_paths(),
        [
            Utf8PathBuf::from("/proj/myproject/broken.py"),
            Utf8PathBuf::from("/proj/myproject/settings.py"),
        ]
    );
}

#[test]
fn settings_sources_includes_import_after_loop_guard_change() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/myproject/settings.py",
                "DEBUG = False\nfor plugin in PLUGINS:\n    DEBUG = True\nif DEBUG:\n    from .broken import *\nelse:\n    INSTALLED_APPS = ['local']\n",
            ),
            ("/proj/myproject/broken.py", "INSTALLED_APPS = [\n"),
        ],
    );

    let discovery = compute_django_discovery(&db, project);

    assert_eq!(
        discovery.file_paths(),
        [
            Utf8PathBuf::from("/proj/myproject/broken.py"),
            Utf8PathBuf::from("/proj/myproject/settings.py"),
        ]
    );
}

#[test]
fn settings_sources_plain_import_alias_makes_guarded_import_reachable() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/myproject/settings.py",
                "DEBUG = False\nimport flags as DEBUG\nif DEBUG:\n    from .broken import *\nelse:\n    INSTALLED_APPS = ['local']\n",
            ),
            ("/proj/myproject/broken.py", "INSTALLED_APPS = [\n"),
        ],
    );

    let discovery = compute_django_discovery(&db, project);

    assert_eq!(
        discovery.file_paths(),
        [
            Utf8PathBuf::from("/proj/myproject/broken.py"),
            Utf8PathBuf::from("/proj/myproject/settings.py"),
        ]
    );
}

#[test]
fn settings_sources_dedupes_duplicate_import_edges() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/myproject/settings.py",
                "from .base import INSTALLED_APPS as FIRST\nfrom .base import INSTALLED_APPS as SECOND\nINSTALLED_APPS = FIRST + SECOND\n",
            ),
            ("/proj/myproject/base.py", "INSTALLED_APPS = ['base']\n"),
        ],
    );

    let discovery = compute_django_discovery(&db, project);

    assert_eq!(
        discovery.file_paths(),
        [
            Utf8PathBuf::from("/proj/myproject/base.py"),
            Utf8PathBuf::from("/proj/myproject/settings.py"),
        ]
    );
}

#[test]
fn imported_unsupported_mutation_marks_setting_partial() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/myproject/settings.py", "from .base import *\n"),
            (
                "/proj/myproject/base.py",
                "STATICFILES_DIRS = ['static']\nSTATICFILES_DIRS.append('extra')\n",
            ),
        ],
    );

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();

    assert_eq!(
        settings["staticfiles"]["staticfiles_dirs"]["extraction"],
        "partial"
    );
}

#[test]
fn cyclic_star_import_marks_imported_setting_partial() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/myproject/settings.py", "from .base import *\n"),
            (
                "/proj/myproject/base.py",
                "STATICFILES_DIRS = ['static']\nfrom .settings import *\n",
            ),
        ],
    );

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();

    assert_eq!(
        settings["staticfiles"]["staticfiles_dirs"]["extraction"],
        "partial"
    );
}

#[test]
fn local_assignment_clears_imported_unsupported_mutation() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/myproject/settings.py",
                "from .base import *\nSTATICFILES_DIRS = []\n",
            ),
            (
                "/proj/myproject/base.py",
                "STATICFILES_DIRS = ['static']\nSTATICFILES_DIRS.append('extra')\n",
            ),
        ],
    );

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();

    assert_eq!(
        settings["staticfiles"]["staticfiles_dirs"]["extraction"],
        "complete"
    );
}

#[test]
fn named_imported_unsupported_mutation_marks_setting_partial() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/myproject/settings.py",
                "from .base import STATICFILES_DIRS\n",
            ),
            (
                "/proj/myproject/base.py",
                "STATICFILES_DIRS = ['static']\nSTATICFILES_DIRS.append('extra')\n",
            ),
        ],
    );

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();

    assert_eq!(
        settings["staticfiles"]["staticfiles_dirs"]["extraction"],
        "partial"
    );
}

#[test]
fn aliased_imported_unsupported_mutation_survives_assignment() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/myproject/settings.py",
                "from .base import STATICFILES_DIRS as BASE_STATICFILES_DIRS\nSTATICFILES_DIRS = BASE_STATICFILES_DIRS\n",
            ),
            (
                "/proj/myproject/base.py",
                "STATICFILES_DIRS = ['static']\nSTATICFILES_DIRS.append('extra')\n",
            ),
        ],
    );

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();

    assert_eq!(
        settings["staticfiles"]["staticfiles_dirs"]["extraction"],
        "partial"
    );
}

#[test]
fn same_name_assignment_preserves_imported_unsupported_mutation() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/myproject/settings.py",
                "from .base import STATICFILES_DIRS\nSTATICFILES_DIRS = STATICFILES_DIRS\n",
            ),
            (
                "/proj/myproject/base.py",
                "STATICFILES_DIRS = ['static']\nSTATICFILES_DIRS.append('extra')\n",
            ),
        ],
    );

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();

    assert_eq!(
        settings["staticfiles"]["staticfiles_dirs"]["extraction"],
        "partial"
    );
}

#[test]
fn django_discovery_includes_deduped_unreadable_settings_source() {
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
        Some(PythonModuleName::parse("myproject.settings").unwrap()),
        pythonpath,
        Vec::new(),
        tag_specs,
    );
    db.set_project(project);

    let settings_sources =
        DiscoveryPhase::ProjectFacts(ProjectFactsPhase::SettingsSources).run(&db, project);
    assert_eq!(settings_sources.count(), 3);

    let discovery = compute_django_discovery(&db, project);

    let expected = [
        Utf8PathBuf::from("/proj/myproject/base.py"),
        Utf8PathBuf::from("/proj/myproject/settings.py"),
        unreadable,
    ];
    assert_eq!(discovery.file_paths(), expected.as_slice());
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
fn template_dirs_resolve_app_config_class_from_init_module() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/something/__init__.py", ""),
            ("/proj/something/templates/something/detail.html", "detail"),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['something.WeirdConfig']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let dirs = template_resolution(&db, project)
        .known_template_dirs(&db)
        .expect("template dirs should be known");

    assert_eq!(dirs, vec![Utf8PathBuf::from("/proj/something/templates")]);
}

#[test]
fn template_dirs_demote_broken_app_config_entry_to_partial() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/myapp/__init__.py", ""),
            ("/proj/myapp/templates/myapp/detail.html", "detail"),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['myapp.apps.MyConfig']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let dirs = template_resolution(&db, project).known_template_dirs(&db);

    assert!(dirs.is_none());
}

#[test]
fn template_context_processors_complete_settings_mark_inventory_complete() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/django/template/__init__.py", ""),
            ("/proj/django/template/context_processors.py", ""),
            (
                "/proj/myproject/settings.py",
                "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'context_processors': ['django.template.context_processors.request']}}]\n",
            ),
        ],
    );

    let processors = template_context_processors(&db, project);

    assert_eq!(processors.status(), TemplateInventoryStatus::Complete);
}

#[test]
fn template_context_processors_invalid_entry_marks_inventory_incomplete() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[(
            "/proj/myproject/settings.py",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'context_processors': ['django.template.context_processors.request', object()]}}]\n",
        )],
    );

    let processors = template_context_processors(&db, project);

    assert_eq!(processors.status(), TemplateInventoryStatus::Incomplete);
}

#[test]
fn template_context_processors_without_usable_settings_are_not_discovered() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj").install(&mut db);

    let processors = template_context_processors(&db, project);

    assert_eq!(processors.status(), TemplateInventoryStatus::NotDiscovered);
}

#[test]
fn template_context_processors_resolve_module_prefix_and_callable_tail() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/django/template/__init__.py", ""),
            ("/proj/django/template/context_processors.py", ""),
            (
                "/proj/myproject/settings.py",
                "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'context_processors': ['django.template.context_processors.request']}}]\n",
            ),
        ],
    );

    let processors = template_context_processors(&db, project);
    let processor = processors.processors().first().unwrap();

    assert_eq!(
        processor.path_str(),
        "django.template.context_processors.request"
    );
    assert_eq!(
        processor.module().map(|module| module.name().as_str()),
        Some("django.template.context_processors")
    );
    assert_eq!(processor.unresolved_tail(), ["request"]);
}

#[test]
fn template_context_processors_keep_imported_origin_file() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings.local",
        &[
            ("/proj/myproject/settings/__init__.py", ""),
            (
                "/proj/myproject/settings/base.py",
                "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'context_processors': ['django.template.context_processors.request']}}]\n",
            ),
            (
                "/proj/myproject/settings/local.py",
                "from .base import TEMPLATES\n",
            ),
        ],
    );

    let processors = template_context_processors(&db, project);
    let processor = processors.processors().first().unwrap();
    let (file, _) = processor.origin();

    assert_eq!(file.path(&db).as_str(), "/proj/myproject/settings/base.py");
}

#[test]
fn template_context_processors_keep_unresolved_facts_with_tail() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[(
            "/proj/myproject/settings.py",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'context_processors': ['missing.context.processor']}}]\n",
        )],
    );

    let processors = template_context_processors(&db, project);
    let processor = processors.processors().first().unwrap();

    assert_eq!(processor.path_str(), "missing.context.processor");
    assert!(processor.module().is_none());
    assert_eq!(
        processor.unresolved_tail(),
        ["missing", "context", "processor"]
    );
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
        &[Utf8PathBuf::from("/site")],
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
fn template_dirs_resolve_bare_namespace_app_entry() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/nsapp/templates/nsapp/index.html", "index"),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['nsapp']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let dirs = template_resolution(&db, project)
        .known_template_dirs(&db)
        .expect("namespace app template dirs should be known");

    assert_eq!(dirs, vec![Utf8PathBuf::from("/proj/nsapp/templates")]);
}

#[test]
fn template_dirs_resolve_namespace_app_portions_in_root_order() {
    let mut db = TestDatabase::new();
    db.add_file("/proj/nsapp/templates/project.html", "project");
    db.add_file("/vendor/nsapp/templates/vendor.html", "vendor");
    db.add_file(
        "/proj/myproject/settings.py",
        "INSTALLED_APPS = ['nsapp']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
    );
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/proj"),
        &Interpreter::Auto,
        &[Utf8PathBuf::from("/vendor")],
    );
    search_paths.register_roots(&db);
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .search_paths(search_paths)
        .install(&mut db);

    let dirs = template_resolution(&db, project)
        .known_template_dirs(&db)
        .expect("namespace app template dirs should be known");

    assert_eq!(
        dirs,
        vec![
            Utf8PathBuf::from("/proj/nsapp/templates"),
            Utf8PathBuf::from("/vendor/nsapp/templates"),
        ]
    );
}

#[test]
fn template_dirs_demote_file_module_app_to_partial() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/app.py", ""),
            ("/proj/app/templates/app/index.html", "index"),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['app']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
            ),
        ],
    );

    let dirs = template_resolution(&db, project).known_template_dirs(&db);

    assert!(dirs.is_none());
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
            ("/proj/blog/__init__.py", ""),
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
        .installed_library_str("custom")
        .expect("custom library should be discovered");
    assert_eq!(custom.module_name_str(), "blog.templatetags.custom");
    assert!(
        custom
            .symbols()
            .iter()
            .any(|symbol| symbol.name() == "hello")
    );
    assert_eq!(
        active_builtin_modules(libraries),
        vec![
            "django.template.defaulttags",
            "django.template.defaultfilters",
            "django.template.loader_tags",
        ]
    );
}

#[test]
fn template_libraries_discover_package_templatetags() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/nsapp/__init__.py", ""),
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
        .installed_library_str("custom")
        .expect("package templatetag should be discovered");
    assert_eq!(custom.module_name_str(), "nsapp.templatetags.custom");
    assert!(
        custom
            .symbols()
            .iter()
            .any(|symbol| symbol.name() == "hello")
    );
}

#[test]
fn template_libraries_discover_namespace_package_templatetags() {
    let mut db = TestDatabase::new();
    db.add_file("/proj/nsapp/other.py", "");
    db.add_file("/vendor/nsapp/templatetags/__init__.py", "");
    db.add_file(
        "/vendor/nsapp/templatetags/custom.py",
        "from django import template\nregister = template.Library()\n@register.simple_tag\ndef hello():\n    pass\n",
    );
    db.add_file(
        "/proj/myproject/settings.py",
        "INSTALLED_APPS = ['nsapp']\nTEMPLATES = []\n",
    );
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/proj"),
        &Interpreter::Auto,
        &[Utf8PathBuf::from("/vendor")],
    );
    search_paths.register_roots(&db);
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .search_paths(search_paths)
        .install(&mut db);

    let libraries = template_libraries(&db, project);

    let custom = libraries
        .installed_library_str("custom")
        .expect("namespace package templatetag should be discovered");
    assert_eq!(custom.module_name_str(), "nsapp.templatetags.custom");
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
            ("/proj/blog/__init__.py", ""),
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

    let empty = libraries.installed_library_str("empty").unwrap();
    assert_eq!(empty.module_name_str(), "blog.templatetags.empty");
    assert!(empty.symbols().is_empty());
}

#[test]
fn template_libraries_skip_discovered_helpers_without_demoting_inventory() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/django/__init__.py", ""),
            ("/proj/blog/__init__.py", ""),
            ("/proj/blog/templatetags/__init__.py", ""),
            ("/proj/blog/templatetags/helpers.py", "VALUE = 1\n"),
            (
                "/proj/blog/templatetags/orphan.py",
                "@register.simple_tag\ndef orphan():\n    pass\n",
            ),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['blog']\nTEMPLATES = []\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);

    assert!(libraries.inventory_is_complete());
    assert!(libraries.installed_library_str("helpers").is_none());
    let orphan = libraries
        .installed_library_str("orphan")
        .expect("symbol-bearing modules are template libraries even without register assignment");
    assert!(
        orphan
            .symbols()
            .iter()
            .any(|symbol| symbol.name() == "orphan")
    );
}

#[test]
fn template_libraries_failed_discovered_library_marks_inventory_incomplete() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/django/__init__.py", ""),
            ("/proj/blog/templatetags/__init__.py", ""),
            ("/proj/blog/templatetags/broken.py", "def broken(:\n"),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = ['blog']\nTEMPLATES = []\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);

    assert!(libraries.inventory_may_omit_loaded_symbols());
    assert!(libraries.installed_library_str("broken").is_none());
}

#[test]
fn template_libraries_broad_invalid_identifier_marks_inventory_incomplete() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            ("/proj/django/__init__.py", ""),
            ("/proj/crispy/__init__.py", ""),
            ("/proj/crispy/templatetags/__init__.py", ""),
            ("/proj/crispy/templatetags/bad-name.py", "VALUE = 1\n"),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = []\nTEMPLATES = []\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);

    assert!(libraries.inventory_may_omit_loaded_symbols());
}

#[test]
fn template_libraries_failed_available_candidate_walk_marks_inventory_incomplete() {
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(Utf8PathBuf::from("/proj/django/__init__.py"), String::new());
    fs.add_file(
        Utf8PathBuf::from("/proj/myproject/settings.py"),
        "INSTALLED_APPS = []\nTEMPLATES = []\n".to_string(),
    );
    let mut db = OsTestDatabase::with_file_system(Arc::new(FailingWalkFileSystem {
        inner: fs,
        failing_root: Utf8PathBuf::from("/proj"),
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
        Some(PythonModuleName::parse("myproject.settings").unwrap()),
        pythonpath,
        Vec::new(),
        tag_specs,
    );
    db.set_project(project);

    let libraries = template_libraries(&db, project);

    assert!(libraries.inventory_may_omit_loaded_symbols());
}

#[test]
fn template_libraries_collect_available_uninstalled_templatetags() {
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

    let UnknownLibraryOutcome::AvailableInApps { primary_app, apps } =
        libraries.unknown_library_outcome(&library_name("crispy"))
    else {
        panic!("crispy should be reported as an available-app library candidate");
    };
    assert_eq!(primary_app.as_str(), "crispy");
    assert_eq!(
        apps.iter()
            .map(PythonModuleName::as_str)
            .collect::<Vec<_>>(),
        vec!["crispy"]
    );

    let UnknownSymbolOutcome::Available { app, load_name } =
        libraries.unknown_tag_outcome("crispy_tag")
    else {
        panic!("crispy_tag should be reported as an available-app tag candidate");
    };
    assert_eq!(app.as_str(), "crispy");
    assert_eq!(load_name.as_str(), "crispy");

    let UnknownSymbolOutcome::Available { app, load_name } =
        libraries.unknown_filter_outcome("crispy_filter")
    else {
        panic!("crispy_filter should be reported as an available-app filter candidate");
    };
    assert_eq!(app.as_str(), "crispy");
    assert_eq!(load_name.as_str(), "crispy");

    assert_eq!(
        libraries.unknown_library_outcome(&library_name("myapp_tags")),
        UnknownLibraryOutcome::Suppressed,
        "installed app libraries must be subtracted from available candidates"
    );
}

#[test]
fn template_libraries_available_candidates_rerun_after_search_root_revision_bump() {
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

    assert_eq!(
        template_libraries(&db, project).unknown_library_outcome(&library_name("crispy")),
        UnknownLibraryOutcome::TrulyUnknown
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

    let libraries = template_libraries(&db, project);

    assert!(matches!(
        libraries.unknown_library_outcome(&library_name("crispy")),
        UnknownLibraryOutcome::AvailableInApps { .. }
    ));
}

#[test]
fn django_discovery_updates_available_template_library_symbols() {
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

    assert_eq!(
        template_libraries(&db, project).unknown_tag_outcome("new_tag"),
        UnknownSymbolOutcome::TrulyUnknown
    );

    db.add_file(
        "/proj/crispy/templatetags/crispy.py",
        "from django import template\nregister = template.Library()\n@register.simple_tag\ndef new_tag():\n    pass\n",
    );
    apply_project_discovery(&mut db);

    assert!(matches!(
        template_libraries(&db, project).unknown_tag_outcome("new_tag"),
        UnknownSymbolOutcome::Available { .. }
    ));
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

    let custom = libraries.installed_library_str("custom").unwrap();
    assert_eq!(custom.module_name_str(), "custom_tags");
    assert!(
        custom
            .symbols()
            .iter()
            .any(|symbol| symbol.name() == "configured")
    );
    assert!(
        libraries
            .template_symbol_candidates(TemplateSymbolKind::Filter)
            .iter()
            .any(|candidate| {
                candidate.symbol.name() == "configured_filter"
                    && matches!(
                        &candidate.availability,
                        TemplateSymbolAvailability::Builtin { module }
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
    let custom = libraries.installed_library_str("custom").unwrap();
    assert_eq!(custom.module_name_str(), "project_tags");
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
            ("/proj/blog/__init__.py", ""),
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

    let custom = libraries.installed_library_str("custom").unwrap();
    assert_eq!(custom.module_name_str(), "project_tags");
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
    assert_eq!(
        libraries.unknown_tag_outcome("old_tag"),
        UnknownSymbolOutcome::TrulyUnknown,
        "a configured alias can shadow an installed app library without making that app available"
    );
}

#[test]
fn template_libraries_failed_configured_library_marks_inventory_incomplete() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/django/template/defaulttags.py",
                "from django import template\nregister = template.Library()\n",
            ),
            (
                "/proj/django/template/defaultfilters.py",
                "from django import template\nregister = template.Library()\n",
            ),
            (
                "/proj/django/template/loader_tags.py",
                "from django import template\nregister = template.Library()\n",
            ),
            ("/proj/broken_tags.py", "def broken(:\n"),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'broken': 'broken_tags'}}}]\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);

    assert!(libraries.inventory_may_omit_loaded_symbols());
    assert!(libraries.installed_library_str("broken").is_none());
}

#[test]
fn template_libraries_omit_invalid_configured_alias_and_demote_knowledge() {
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
    assert!(libraries.installed_library_str("broken").is_none());
}

#[test]
fn template_libraries_omit_missing_configured_alias_and_demote_knowledge() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[(
            "/proj/myproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'missing': 'missing_tags'}}}]\n",
        )],
    );

    let libraries = template_libraries(&db, project);

    assert!(libraries.inventory_may_omit_loaded_symbols());
    assert!(libraries.installed_library_str("missing").is_none());
}

#[test]
fn template_libraries_omit_configured_non_library_module_and_demote_knowledge() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/django/template/defaulttags.py",
                "from django import template\nregister = template.Library()\n",
            ),
            (
                "/proj/django/template/defaultfilters.py",
                "from django import template\nregister = template.Library()\n",
            ),
            (
                "/proj/django/template/loader_tags.py",
                "from django import template\nregister = template.Library()\n",
            ),
            ("/proj/not_a_library.py", "VALUE = 1\n"),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'not_a_library'}}}]\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);

    assert!(libraries.inventory_may_omit_loaded_symbols());
    assert!(libraries.installed_library_str("custom").is_none());
}

#[test]
fn template_libraries_active_libraries_include_only_resolved_libraries() {
    let mut db = TestDatabase::new();
    let project = project_with_settings(
        &mut db,
        "myproject.settings",
        &[
            (
                "/proj/good_tags.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef good():\n    pass\n",
            ),
            ("/proj/not_a_library.py", "VALUE = 1\n"),
            (
                "/proj/myproject/settings.py",
                "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'good': 'good_tags', 'missing': 'missing_tags', 'invalid': 'bad-module'}, 'builtins': ['not_a_library']}}]\n",
            ),
        ],
    );

    let libraries = template_libraries(&db, project);
    let active_modules: Vec<_> = libraries
        .active_libraries()
        .map(|library| library.module_name_str().to_string())
        .collect();

    assert_eq!(active_modules, vec!["good_tags"]);
    assert!(libraries.inventory_may_omit_loaded_symbols());
    assert!(libraries.installed_library_str("good").is_some());
    assert!(libraries.installed_library_str("missing").is_none());
    assert!(libraries.installed_library_str("invalid").is_none());
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
    db.set_project(project);

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
    db.set_project(project);

    let libraries = template_libraries(&db, project);
    let actual_builtins = active_builtin_modules(libraries);
    assert_eq!(actual_builtins, golden.template_libraries.builtins);

    let actual_libraries: BTreeMap<_, _> = libraries
        .completion_library_names()
        .into_iter()
        .filter_map(|name| {
            let library = libraries.installed_library(&name)?;
            Some((
                name.as_str().to_string(),
                library.module_name_str().to_string(),
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

    for library in libraries.active_libraries() {
        let load_name = library.load_name().map(|name| name.as_str().to_string());

        for symbol in library.symbols() {
            symbols.push(GoldenTemplateSymbol {
                kind: symbol.kind,
                name: symbol.name().to_string(),
                load_name: load_name.clone(),
                library_module: library.module_name_str().to_string(),
                module: library.module_name_str().to_string(),
            });
        }
    }

    symbols
}

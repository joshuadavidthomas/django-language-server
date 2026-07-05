use std::collections::BTreeMap;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_project::testing::compute_django_discovery;
use djls_project::testing::model_modules;
use djls_project::*;
use djls_source::Db as _;
use djls_source::FileRootKind;
use djls_source::InMemoryFileSystem;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;

struct TestModel {
    module_name: PythonModuleName,
}

#[derive(Default)]
struct TestModelGraph {
    models: BTreeMap<String, TestModel>,
}

impl TestModelGraph {
    fn get(&self, name: &str) -> Option<&TestModel> {
        self.models.get(name)
    }
}

fn compute_model_graph(db: &TestDatabase, project: Project) -> TestModelGraph {
    let mut graph = TestModelGraph::default();
    for module in model_modules(db, project).iter().rev() {
        let source = module.file().source(db);
        for line in source.as_str().lines() {
            let Some(rest) = line.trim_start().strip_prefix("class ") else {
                continue;
            };
            let name = rest.split(['(', ':']).next().unwrap_or("").trim();
            if name.is_empty() {
                continue;
            }
            graph.models.insert(
                name.to_string(),
                TestModel {
                    module_name: module.name().clone(),
                },
            );
        }
    }
    graph
}

fn project_for_search_paths(
    db: &mut TestDatabase,
    root: &str,
    search_paths: SearchPaths,
) -> Project {
    ProjectFixture::new(root)
        .search_paths(search_paths)
        .interpreter(Interpreter::Auto)
        .register_roots(false)
        .build(db)
}

fn project_with_template_settings(
    db: &mut TestDatabase,
    root: &str,
    search_paths: SearchPaths,
    settings_source: impl Into<String>,
) -> Project {
    ProjectFixture::new(root)
        .search_paths(search_paths)
        .interpreter(Interpreter::Auto)
        .register_roots(false)
        .django_settings_module("settings")
        .file(format!("{root}/settings.py"), settings_source)
        .build(db)
}

fn apply_project_discovery(db: &mut TestDatabase) {
    let project = db.project().expect("project should be configured");
    let discovery = compute_django_discovery(db, project);
    apply_django_discovery(db, discovery);
}

fn django_template_settings(installed_apps: &[&str], builtins: &[&str]) -> String {
    let installed_apps = installed_apps
        .iter()
        .map(|app| format!("'{app}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let builtins = builtins
        .iter()
        .map(|module| format!("'{module}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "INSTALLED_APPS = [{installed_apps}]\n\
         TEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', \
         'DIRS': [], 'APP_DIRS': True, 'OPTIONS': {{'builtins': [{builtins}]}}}}]\n"
    )
}

#[test]
fn search_paths_detect_top_level_src_after_project_root() {
    let mut fs = InMemoryFileSystem::new();
    fs.add_file("/project/src/app.py".into(), String::new());

    let search_paths =
        SearchPaths::from_project_settings(&fs, Utf8Path::new("/project"), &Interpreter::Auto, &[]);
    let paths: Vec<_> = search_paths.iter().cloned().collect();

    assert_eq!(
        paths,
        vec![
            SearchPath::FirstParty(Utf8PathBuf::from("/project")),
            SearchPath::FirstParty(Utf8PathBuf::from("/project/src")),
        ]
    );
}

#[test]
fn search_paths_do_not_detect_top_level_src_when_absent() {
    let mut fs = InMemoryFileSystem::new();
    fs.add_file("/project/app.py".into(), String::new());

    let search_paths =
        SearchPaths::from_project_settings(&fs, Utf8Path::new("/project"), &Interpreter::Auto, &[]);
    let paths: Vec<_> = search_paths.iter().cloned().collect();

    assert_eq!(
        paths,
        vec![SearchPath::FirstParty(Utf8PathBuf::from("/project"))]
    );
}

#[test]
fn search_paths_add_simple_pth_entries_as_editable_roots() {
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(
        "/project/.venv/lib/python3.12/site-packages/django/__init__.py".into(),
        String::new(),
    );
    fs.add_file(
        "/project/.venv/lib/python3.12/site-packages/editable_relative/pkg.py".into(),
        String::new(),
    );
    fs.add_file("/editable_absolute/pkg.py".into(), String::new());
    fs.add_file(
        "/project/.venv/lib/python3.12/site-packages/editable.pth".into(),
        "# comment\n\nimport site\neditable_relative\n/editable_absolute\nmissing\n".to_string(),
    );

    let search_paths =
        SearchPaths::from_project_settings(&fs, Utf8Path::new("/project"), &Interpreter::Auto, &[]);
    let paths: Vec<_> = search_paths.iter().cloned().collect();

    assert_eq!(
        paths,
        vec![
            SearchPath::FirstParty(Utf8PathBuf::from("/project")),
            SearchPath::SitePackages(Utf8PathBuf::from(
                "/project/.venv/lib/python3.12/site-packages"
            )),
            SearchPath::Editable(Utf8PathBuf::from(
                "/project/.venv/lib/python3.12/site-packages/editable_relative"
            )),
            SearchPath::Editable(Utf8PathBuf::from("/editable_absolute")),
        ]
    );
}

#[test]
fn search_paths_skip_pth_entries_that_duplicate_existing_roots() {
    let mut fs = InMemoryFileSystem::new();
    fs.add_file("/project/src/app.py".into(), String::new());
    fs.add_file(
        "/project/.venv/lib/python3.12/site-packages/django/__init__.py".into(),
        String::new(),
    );
    fs.add_file(
        "/project/.venv/lib/python3.12/site-packages/editable.pth".into(),
        "/project/src\n".to_string(),
    );

    let search_paths =
        SearchPaths::from_project_settings(&fs, Utf8Path::new("/project"), &Interpreter::Auto, &[]);
    let paths: Vec<_> = search_paths.iter().cloned().collect();

    assert_eq!(
        paths,
        vec![
            SearchPath::FirstParty(Utf8PathBuf::from("/project")),
            SearchPath::FirstParty(Utf8PathBuf::from("/project/src")),
            SearchPath::SitePackages(Utf8PathBuf::from(
                "/project/.venv/lib/python3.12/site-packages"
            )),
        ]
    );
}

#[test]
fn search_paths_keep_site_packages_external_inside_project_root() {
    let mut fs = InMemoryFileSystem::new();
    fs.add_file("/project/src/app/__init__.py".into(), String::new());
    fs.add_file("/outside/pkg/__init__.py".into(), String::new());
    fs.add_file(
        "/project/.venv/lib/python3.12/site-packages/django/__init__.py".into(),
        String::new(),
    );

    let pythonpath = vec![
        Utf8PathBuf::from("/project/src"),
        Utf8PathBuf::from("/outside"),
        Utf8PathBuf::from("/project/.venv/lib/python3.12/site-packages"),
    ];
    let search_paths = SearchPaths::from_project_settings(
        &fs,
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &pythonpath,
    );

    let paths: Vec<_> = search_paths.iter().map(SearchPath::path).collect();
    assert_eq!(
        paths,
        vec![
            Utf8Path::new("/project"),
            Utf8Path::new("/project/src"),
            Utf8Path::new("/outside"),
            Utf8Path::new("/project/.venv/lib/python3.12/site-packages"),
        ]
    );

    let external_paths: Vec<_> = search_paths
        .iter()
        .filter(|search_path| !matches!(search_path, SearchPath::FirstParty(_)))
        .map(SearchPath::path)
        .collect();
    assert_eq!(
        external_paths,
        vec![
            Utf8Path::new("/outside"),
            Utf8Path::new("/project/.venv/lib/python3.12/site-packages"),
        ]
    );
}

#[cfg(target_os = "windows")]
#[test]
fn search_paths_find_windows_style_venv_site_packages() {
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(
        "/project/.venv/Lib/site-packages/django/__init__.py".into(),
        String::new(),
    );

    let search_paths =
        SearchPaths::from_project_settings(&fs, Utf8Path::new("/project"), &Interpreter::Auto, &[]);

    let paths: Vec<_> = search_paths.iter().map(SearchPath::path).collect();
    assert_eq!(
        paths,
        vec![
            Utf8Path::new("/project"),
            Utf8Path::new("/project/.venv/Lib/site-packages"),
        ]
    );
}

#[test]
fn model_modules_use_first_party_search_path_relative_names() {
    let mut db = TestDatabase::new();
    db.add_file(
        "/project/src/blog/models.py",
        "from django.db import models\nclass Article(models.Model):\n    pass\n",
    );

    let pythonpath = vec![Utf8PathBuf::from("/project/src")];
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &pythonpath,
    );
    search_paths.register_roots(&db);
    let project = project_for_search_paths(&mut db, "/project", search_paths);

    let modules = model_modules(&db, project);
    assert!(
        modules
            .iter()
            .any(|module| module.name().as_str() == "blog.models")
    );
    assert!(
        !modules
            .iter()
            .any(|module| module.name().as_str() == "src.blog.models")
    );
}

#[test]
fn registering_search_paths_removes_obsolete_external_roots() {
    let db = TestDatabase::new();
    db.add_file("/external/pkg/models.py", "");

    let pythonpath = vec![Utf8PathBuf::from("/external")];
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &pythonpath,
    );
    search_paths.register_roots(&db);
    let external_root = db
        .files()
        .expect_root(&db, Utf8Path::new("/external/pkg/models.py"));
    assert_eq!(external_root.kind(&db), FileRootKind::Project);

    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &[],
    );
    search_paths.register_roots(&db);
    assert!(
        db.files()
            .root(&db, Utf8Path::new("/external/pkg/models.py"))
            .is_none()
    );
}

#[test]
fn model_modules_tolerate_unregistered_search_paths() {
    let mut db = TestDatabase::new();
    db.add_file(
        "/shared/blog/models.py",
        "from django.db import models\nclass SharedArticle(models.Model):\n    pass\n",
    );

    let pythonpath = vec![Utf8PathBuf::from("/shared")];
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &pythonpath,
    );
    let project = project_for_search_paths(&mut db, "/project", search_paths);

    let modules = model_modules(&db, project);
    assert!(
        modules
            .iter()
            .any(|module| module.name().as_str() == "blog.models")
    );
}

#[test]
fn template_libraries_tolerate_unregistered_search_paths() {
    let mut db = TestDatabase::new();
    db.add_file("/project/django/templatetags/__init__.py", "");
    db.add_file(
        "/project/django/templatetags/i18n.py",
        "from django import template\nregister = template.Library()\n@register.simple_tag\ndef trans():\n    pass\n",
    );

    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &[],
    );
    let project = project_with_template_settings(
        &mut db,
        "/project",
        search_paths,
        django_template_settings(&[], &[]),
    );

    let libraries = template_libraries(&db, project);
    let active: Vec<_> = libraries.active_libraries().collect();

    assert_eq!(active.len(), 1);
    assert_eq!(active[0].module_name().as_str(), "django.templatetags.i18n");
}

#[test]
fn template_library_resolution_uses_project_venv_site_packages_root() {
    let mut db = TestDatabase::new();
    db.add_file(
        "/project/.venv/lib/python3.12/site-packages/django/templatetags/__init__.py",
        "",
    );
    db.add_file(
        "/project/.venv/lib/python3.12/site-packages/django/templatetags/i18n.py",
        "from django import template\nregister = template.Library()\n@register.simple_tag\ndef trans():\n    pass\n",
    );

    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &[],
    );
    search_paths.register_roots(&db);
    let project = project_with_template_settings(
        &mut db,
        "/project",
        search_paths,
        django_template_settings(&[], &[]),
    );

    let libraries = template_libraries(&db, project);
    let active: Vec<_> = libraries.active_libraries().collect();

    assert_eq!(active.len(), 1);
    assert_eq!(active[0].module_name().as_str(), "django.templatetags.i18n");
    let root = db.files().expect_root(&db, active[0].file().path(&db));
    assert_eq!(root.kind(&db), FileRootKind::SearchPath);
}

#[test]
fn template_library_resolution_prefers_first_party_module_shadowing_dependency() {
    let mut db = TestDatabase::new();
    db.add_file("/project/django/templatetags/__init__.py", "");
    db.add_file(
        "/project/django/templatetags/i18n.py",
        "from django import template\nregister = template.Library()\n@register.simple_tag\ndef trans():\n    pass\n",
    );
    db.add_file(
        "/project/.venv/lib/python3.12/site-packages/django/templatetags/__init__.py",
        "",
    );
    db.add_file(
        "/project/.venv/lib/python3.12/site-packages/django/templatetags/i18n.py",
        "from django import template\nregister = template.Library()\n@register.simple_tag\ndef trans():\n    pass\n",
    );

    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &[],
    );
    search_paths.register_roots(&db);
    let project = project_with_template_settings(
        &mut db,
        "/project",
        search_paths,
        django_template_settings(&[], &[]),
    );

    let libraries = template_libraries(&db, project);
    let active: Vec<_> = libraries.active_libraries().collect();

    assert_eq!(active.len(), 1);
    assert_eq!(
        active[0].file().path(&db),
        Utf8Path::new("/project/django/templatetags/i18n.py")
    );
}

#[test]
fn active_template_libraries_preserve_builtin_order_across_roots() {
    let mut db = TestDatabase::new();
    db.add_file(
        "/project/a/templatetags/tags.py",
        "from django import template\nregister = template.Library()\n@register.simple_tag\ndef a_tag():\n    pass\n",
    );
    db.add_file(
        "/project/.venv/lib/python3.12/site-packages/z/templatetags/tags.py",
        "from django import template\nregister = template.Library()\n@register.simple_tag\ndef z_tag():\n    pass\n",
    );

    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &[],
    );
    search_paths.register_roots(&db);
    let project = project_with_template_settings(
        &mut db,
        "/project",
        search_paths,
        django_template_settings(&[], &["a.templatetags.tags", "z.templatetags.tags"]),
    );

    let libraries = template_libraries(&db, project);
    let module_names: Vec<_> = libraries
        .active_libraries()
        .map(|library| library.module_name().as_str().to_string())
        .collect();

    assert_eq!(
        module_names,
        vec!["a.templatetags.tags", "z.templatetags.tags"]
    );
}

#[test]
fn active_template_libraries_yield_installed_before_builtins() {
    let mut db = TestDatabase::new();
    db.add_file(
        "/project/installed_tags.py",
        "from django import template\nregister = template.Library()\n@register.simple_tag\ndef custom():\n    pass\n",
    );
    db.add_file(
        "/project/builtin_tags.py",
        "from django import template\nregister = template.Library()\n@register.simple_tag\ndef builtin():\n    pass\n",
    );

    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &[],
    );
    search_paths.register_roots(&db);
    let project = project_with_template_settings(
        &mut db,
        "/project",
        search_paths,
        "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'installed_tags'}, 'builtins': ['builtin_tags']}}]\n",
    );

    let libraries = template_libraries(&db, project);
    let module_names: Vec<_> = libraries
        .active_libraries()
        .map(|library| library.module_name().as_str().to_string())
        .collect();

    assert_eq!(module_names, vec!["installed_tags", "builtin_tags"]);
}

#[test]
fn builtin_template_libraries_preserve_order_across_roots() {
    let mut db = TestDatabase::new();
    db.add_file(
        "/project/z_first.py",
        r"
from django import template
register = template.Library()

@register.filter
def duplicate(value):
    return value
",
    );
    db.add_file(
        "/project/.venv/lib/python3.12/site-packages/a_second.py",
        r"
from django import template
register = template.Library()

@register.filter
def duplicate(value, arg):
    return value
",
    );

    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &[],
    );
    search_paths.register_roots(&db);
    let project = project_with_template_settings(
        &mut db,
        "/project",
        search_paths,
        django_template_settings(&[], &["z_first", "a_second"]),
    );

    let libraries = template_libraries(&db, project);
    let module_names: Vec<_> = libraries
        .active_libraries()
        .map(|library| library.module_name().as_str().to_string())
        .collect();

    assert_eq!(module_names, vec!["z_first", "a_second"]);
}

#[test]
fn project_model_graph_reads_changed_project_file_after_django_discovery() {
    let mut db = TestDatabase::new();
    db.add_file(
        "/project/blog/models.py",
        "from django.db import models\nclass Article(models.Model):\n    pass\n",
    );

    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &[],
    );
    search_paths.register_roots(&db);
    let project = project_for_search_paths(&mut db, "/project", search_paths);
    db.set_project(project);

    let graph = compute_model_graph(&db, project);
    assert!(graph.get("Article").is_some());
    assert!(graph.get("Comment").is_none());

    db.add_file(
        "/project/blog/models.py",
        "from django.db import models\nclass Comment(models.Model):\n    pass\n",
    );
    apply_project_discovery(&mut db);

    let graph = compute_model_graph(&db, project);
    assert!(graph.get("Article").is_none());
    assert!(graph.get("Comment").is_some());
}

#[test]
fn project_model_discovery_updates_through_django_discovery() {
    let mut db = TestDatabase::new();
    db.add_file(
        "/project/blog/models.py",
        "from django.db import models\nclass Article(models.Model):\n    pass\n",
    );

    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &[],
    );
    search_paths.register_roots(&db);
    let project = project_for_search_paths(&mut db, "/project", search_paths);
    db.set_project(project);

    let graph = compute_model_graph(&db, project);
    assert!(graph.get("Article").is_some());
    assert!(graph.get("Comment").is_none());

    db.add_file(
        "/project/comments/models.py",
        "from django.db import models\nclass Comment(models.Model):\n    pass\n",
    );
    apply_project_discovery(&mut db);

    let graph = compute_model_graph(&db, project);
    assert!(graph.get("Article").is_some());
    assert!(graph.get("Comment").is_some());
}

#[test]
fn external_model_graph_reads_changed_site_packages_file_after_django_discovery() {
    let mut db = TestDatabase::new();
    db.add_file(
        "/project/.venv/lib/python3.12/site-packages/blog/models.py",
        "from django.db import models\nclass Article(models.Model):\n    pass\n",
    );

    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &[],
    );
    search_paths.register_roots(&db);
    let project = project_for_search_paths(&mut db, "/project", search_paths);
    db.set_project(project);

    let graph = compute_model_graph(&db, project);
    assert!(graph.get("Article").is_some());
    assert!(graph.get("Comment").is_none());

    db.add_file(
        "/project/.venv/lib/python3.12/site-packages/blog/models.py",
        "from django.db import models\nclass Comment(models.Model):\n    pass\n",
    );
    apply_project_discovery(&mut db);

    let graph = compute_model_graph(&db, project);
    assert!(graph.get("Article").is_none());
    assert!(graph.get("Comment").is_some());
}

#[test]
fn external_model_graph_preserves_pythonpath_precedence() {
    let mut db = TestDatabase::new();
    db.add_file(
        "/zfirst/zapp/models.py",
        "from django.db import models\nclass Duplicate(models.Model):\n    pass\n",
    );
    db.add_file(
        "/afallback/aapp/models.py",
        "from django.db import models\nclass Duplicate(models.Model):\n    pass\n",
    );

    let pythonpath = vec![
        Utf8PathBuf::from("/zfirst"),
        Utf8PathBuf::from("/afallback"),
    ];
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &pythonpath,
    );
    search_paths.register_roots(&db);
    let project = project_for_search_paths(&mut db, "/project", search_paths);

    let graph = compute_model_graph(&db, project);
    let model = graph.get("Duplicate").expect("model should be discovered");
    assert_eq!(model.module_name.as_str(), "zapp.models");
}

#[test]
fn external_model_discovery_updates_through_django_discovery() {
    let mut db = TestDatabase::new();
    db.add_file(
        "/project/.venv/lib/python3.12/site-packages/blog/models.py",
        "from django.db import models\nclass Article(models.Model):\n    pass\n",
    );

    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &[],
    );
    search_paths.register_roots(&db);
    let project = project_for_search_paths(&mut db, "/project", search_paths);
    db.set_project(project);

    let graph = compute_model_graph(&db, project);
    assert!(graph.get("Article").is_some());
    assert!(graph.get("Comment").is_none());

    db.add_file(
        "/project/.venv/lib/python3.12/site-packages/comments/models.py",
        "from django.db import models\nclass Comment(models.Model):\n    pass\n",
    );
    apply_project_discovery(&mut db);

    let graph = compute_model_graph(&db, project);
    assert!(graph.get("Article").is_some());
    assert!(graph.get("Comment").is_some());
}

#[test]
fn external_model_discovery_removes_deleted_models_through_django_discovery() {
    let mut db = TestDatabase::new();
    db.add_file(
        "/project/.venv/lib/python3.12/site-packages/blog/models.py",
        "from django.db import models\nclass Article(models.Model):\n    pass\n",
    );
    db.add_file(
        "/project/.venv/lib/python3.12/site-packages/comments/models.py",
        "from django.db import models\nclass Comment(models.Model):\n    pass\n",
    );

    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &[],
    );
    search_paths.register_roots(&db);
    let project = project_for_search_paths(&mut db, "/project", search_paths);
    db.set_project(project);

    let graph = compute_model_graph(&db, project);
    assert!(graph.get("Article").is_some());
    assert!(graph.get("Comment").is_some());

    db.remove_file("/project/.venv/lib/python3.12/site-packages/comments/models.py");
    apply_project_discovery(&mut db);

    let graph = compute_model_graph(&db, project);
    assert!(graph.get("Article").is_some());
    assert!(graph.get("Comment").is_none());
}

#[test]
fn external_model_graph_reads_extra_pythonpath_models() {
    let mut db = TestDatabase::new();
    db.add_file(
        "/shared/blog/models.py",
        "from django.db import models\nclass SharedArticle(models.Model):\n    pass\n",
    );

    let pythonpath = vec![Utf8PathBuf::from("/shared")];
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &pythonpath,
    );
    search_paths.register_roots(&db);
    let project = project_for_search_paths(&mut db, "/project", search_paths);

    let graph = compute_model_graph(&db, project);
    assert!(graph.get("SharedArticle").is_some());
}

#[test]
fn django_discovery_discovers_site_packages_created_after_bootstrap() {
    let mut db = TestDatabase::new();
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &[],
    );
    search_paths.register_roots(&db);
    let project = ProjectFixture::new("/project")
        .search_paths(search_paths)
        .interpreter(Interpreter::Auto)
        .register_roots(false)
        .install(&mut db);

    assert!(
        project
            .search_paths(&db)
            .iter()
            .all(|search_path| search_path.path()
                != Utf8Path::new("/project/.venv/lib/python3.12/site-packages"))
    );

    db.add_file(
        "/project/.venv/lib/python3.12/site-packages/blog/models.py",
        "from django.db import models\nclass VenvArticle(models.Model):\n    pass\n",
    );

    apply_project_discovery(&mut db);

    assert!(project.search_paths(&db).iter().any(|search_path| {
        search_path.path() == Utf8Path::new("/project/.venv/lib/python3.12/site-packages")
    }));
    let graph = compute_model_graph(&db, project);
    assert!(graph.get("VenvArticle").is_some());
}

#[test]
fn compute_and_apply_django_discovery_discovers_site_packages_created_after_bootstrap() {
    let mut db = TestDatabase::new();
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &[],
    );
    search_paths.register_roots(&db);
    let project = ProjectFixture::new("/project")
        .search_paths(search_paths)
        .interpreter(Interpreter::Auto)
        .register_roots(false)
        .install(&mut db);

    db.add_file(
        "/project/.venv/lib/python3.12/site-packages/blog/models.py",
        "from django.db import models\nclass VenvArticle(models.Model):\n    pass\n",
    );

    apply_project_discovery(&mut db);

    assert!(project.search_paths(&db).iter().any(|search_path| {
        search_path.path() == Utf8Path::new("/project/.venv/lib/python3.12/site-packages")
    }));
    let graph = compute_model_graph(&db, project);
    assert!(graph.get("VenvArticle").is_some());
}

#[test]
fn django_discovery_enumerates_new_empty_templatetag_candidate_before_root_bump() {
    let mut db = TestDatabase::new();
    db.add_file("/project/blog/__init__.py", "");
    db.add_file("/project/blog/templatetags/__init__.py", "");

    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &[],
    );
    search_paths.register_roots(&db);
    let project = project_for_search_paths(&mut db, "/project", search_paths);

    let candidates =
        DiscoveryPhase::ProjectFacts(ProjectFactsPhase::TemplateTagCandidates).run(&db, project);
    assert_eq!(candidates.count(), 0);

    db.add_file("/project/blog/templatetags/future.py", "");

    let candidates =
        DiscoveryPhase::ProjectFacts(ProjectFactsPhase::TemplateTagCandidates).run(&db, project);
    assert_eq!(candidates.count(), 1);
}

#[test]
fn model_modules_finds_models_py_without_inspecting_contents() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/emptyapp/models.py", "# no models here\n")
        .build(&db);

    let modules = model_modules(&db, project);
    assert_eq!(modules.len(), 1);
    assert_eq!(modules[0].name().as_str(), "emptyapp.models");
    assert!(modules[0].path().ends_with("models.py"));
}

#[test]
fn model_modules_finds_nested_apps() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/blog/models.py",
            "from django.db import models\nclass BlogModel(models.Model):\n    pass\n",
        )
        .file(
            "/project/accounts/models.py",
            "from django.db import models\nclass AccountsModel(models.Model):\n    pass\n",
        )
        .build(&db);

    let modules = model_modules(&db, project);
    assert_eq!(modules.len(), 2);
    let module_names: Vec<&str> = modules
        .iter()
        .map(|module| module.name().as_str())
        .collect();
    assert!(module_names.contains(&"blog.models"));
    assert!(module_names.contains(&"accounts.models"));
}

#[test]
fn model_modules_finds_models_package_files() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/myapp/models/__init__.py",
            "from .user import User\nfrom .order import Order\n",
        )
        .file(
            "/project/myapp/models/user.py",
            "from django.db import models\nclass User(models.Model):\n    pass\n",
        )
        .file(
            "/project/myapp/models/order.py",
            "from django.db import models\nclass Order(models.Model):\n    user = models.ForeignKey(User, on_delete=models.CASCADE)\n",
        )
        .build(&db);

    let modules = model_modules(&db, project);
    assert_eq!(modules.len(), 3);
    let module_names: Vec<&str> = modules
        .iter()
        .map(|module| module.name().as_str())
        .collect();
    assert!(module_names.contains(&"myapp.models"));
    assert!(module_names.contains(&"myapp.models.user"));
    assert!(module_names.contains(&"myapp.models.order"));
}

#[test]
fn python_module_resolve_prefers_regular_package_over_sibling_file() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app.py", "")
        .file("/project/app/__init__.py", "")
        .build(&db);

    let module = PythonModule::resolve(&db, project, PythonModuleName::parse("app").unwrap())
        .expect("app should resolve");

    assert_eq!(module.path(), Utf8Path::new("/project/app/__init__.py"));
}

#[test]
fn python_module_resolve_uses_first_regular_hit_across_roots() {
    let mut db = TestDatabase::new();
    db.add_file("/project/app.py", "");
    db.add_file("/project/vendor/app/__init__.py", "");

    let pythonpath = vec![Utf8PathBuf::from("/project/vendor")];
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &pythonpath,
    );
    search_paths.register_roots(&db);
    let project = project_for_search_paths(&mut db, "/project", search_paths);

    let module = PythonModule::resolve(&db, project, PythonModuleName::parse("app").unwrap())
        .expect("app should resolve");

    assert_eq!(module.path(), Utf8Path::new("/project/app.py"));
}

#[test]
fn python_module_resolve_returns_none_for_namespace_only_directory() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/views.py", "")
        .build(&db);

    let module = PythonModule::resolve(&db, project, PythonModuleName::parse("app").unwrap());

    assert!(module.is_none());
}

#[test]
fn resolve_module_detail_lists_shadowed_regular_candidates_in_root_order() {
    let mut db = TestDatabase::new();
    db.add_file("/project/app.py", "");
    db.add_file("/project/vendor/app.py", "");

    let pythonpath = vec![Utf8PathBuf::from("/project/vendor")];
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &pythonpath,
    );
    search_paths.register_roots(&db);
    let project = project_for_search_paths(&mut db, "/project", search_paths);
    let name = PythonModuleName::parse("app").unwrap();

    let module = PythonModule::resolve(&db, project, name.clone()).expect("app should resolve");
    let detail = resolve_module_detail(&db, project, name);

    assert_eq!(module.path(), Utf8Path::new("/project/app.py"));
    assert_eq!(
        detail.selected_root,
        Some(SearchPath::FirstParty(Utf8PathBuf::from("/project")))
    );
    assert_eq!(detail.unresolved_reason, None);
    assert_eq!(
        detail.candidates,
        vec![
            CandidateLocation {
                root: SearchPath::FirstParty(Utf8PathBuf::from("/project")),
                path: Utf8PathBuf::from("/project/app.py"),
                kind: CandidateKind::FileModule,
            },
            CandidateLocation {
                root: SearchPath::FirstParty(Utf8PathBuf::from("/project/vendor")),
                path: Utf8PathBuf::from("/project/vendor/app.py"),
                kind: CandidateKind::FileModule,
            },
        ]
    );
}

#[test]
fn resolve_module_detail_reports_namespace_only() {
    let mut db = TestDatabase::new();
    db.add_file("/project/app/views.py", "");
    db.add_file("/vendor/app/models.py", "");

    let pythonpath = vec![Utf8PathBuf::from("/vendor")];
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &pythonpath,
    );
    search_paths.register_roots(&db);
    let project = project_for_search_paths(&mut db, "/project", search_paths);

    let detail = resolve_module_detail(&db, project, PythonModuleName::parse("app").unwrap());

    assert_eq!(detail.selected_root, None);
    assert_eq!(
        detail.unresolved_reason,
        Some(UnresolvedReason::NamespaceOnly)
    );
    assert_eq!(
        detail.candidates,
        vec![
            CandidateLocation {
                root: SearchPath::FirstParty(Utf8PathBuf::from("/project")),
                path: Utf8PathBuf::from("/project/app"),
                kind: CandidateKind::NamespacePortion,
            },
            CandidateLocation {
                root: SearchPath::Extra(Utf8PathBuf::from("/vendor")),
                path: Utf8PathBuf::from("/vendor/app"),
                kind: CandidateKind::NamespacePortion,
            },
        ]
    );
}

#[test]
fn resolve_module_detail_reports_not_found() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project").build(&db);

    let detail = resolve_module_detail(&db, project, PythonModuleName::parse("missing").unwrap());

    assert_eq!(detail.selected_root, None);
    assert!(detail.candidates.is_empty());
    assert_eq!(detail.unresolved_reason, Some(UnresolvedReason::NotFound));
}

#[test]
fn resolve_package_dirs_returns_regular_package_dir() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/__init__.py", "")
        .build(&db);

    let dirs = resolve_package_dirs(&db, project, PythonModuleName::parse("app").unwrap());

    assert_eq!(dirs.dirs, vec![Utf8PathBuf::from("/project/app")]);
}

#[test]
fn resolve_package_dirs_merges_namespace_portions_in_root_order() {
    let mut db = TestDatabase::new();
    db.add_file("/project/app/views.py", "");
    db.add_file("/vendor/app/models.py", "");

    let pythonpath = vec![Utf8PathBuf::from("/vendor")];
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &pythonpath,
    );
    search_paths.register_roots(&db);
    let project = project_for_search_paths(&mut db, "/project", search_paths);

    let dirs = resolve_package_dirs(&db, project, PythonModuleName::parse("app").unwrap());

    assert_eq!(
        dirs.dirs,
        vec![
            Utf8PathBuf::from("/project/app"),
            Utf8PathBuf::from("/vendor/app"),
        ]
    );
}

#[test]
fn resolve_package_dirs_returns_empty_for_file_module() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app.py", "")
        .file("/project/app/templates/base.html", "")
        .build(&db);

    let dirs = resolve_package_dirs(&db, project, PythonModuleName::parse("app").unwrap());

    assert!(dirs.dirs.is_empty());
}

#[test]
fn module_name_from_init_file() {
    let path = Utf8Path::new("myapp/models/__init__.py");
    let module_name = PythonModuleName::from_relative_source_path(path).unwrap();
    assert_eq!(module_name.as_str(), "myapp.models");
}

#[test]
fn module_name_from_submodule() {
    let path = Utf8Path::new("myapp/models/user.py");
    let module_name = PythonModuleName::from_relative_source_path(path).unwrap();
    assert_eq!(module_name.as_str(), "myapp.models.user");
}

#[test]
fn model_modules_finds_nested_models_package_files() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/myapp/models/__init__.py", "")
        .file("/project/myapp/models/base/__init__.py", "")
        .file(
            "/project/myapp/models/base/abstract.py",
            "from django.db import models\nclass BaseModel(models.Model):\n    class Meta:\n        abstract = True\n",
        )
        .build(&db);

    let modules = model_modules(&db, project);
    let module_names: Vec<&str> = modules
        .iter()
        .map(|module| module.name().as_str())
        .collect();
    assert!(
        module_names.contains(&"myapp.models.base.abstract"),
        "should discover nested model files: got {module_names:?}"
    );
}

#[test]
fn project_model_discovery_skips_registered_non_first_party_paths() {
    let mut db = TestDatabase::new();
    db.add_file(
        "/project/app/models.py",
        "from django.db import models\nclass App(models.Model): pass\n",
    );
    db.add_file(
        "/project/venv/lib/python3.12/site-packages/somelib/models.py",
        "from django.db import models\nclass Lib(models.Model): pass\n",
    );

    let pythonpath = vec![Utf8PathBuf::from(
        "/project/venv/lib/python3.12/site-packages",
    )];
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::Auto,
        &pythonpath,
    );
    search_paths.register_roots(&db);
    let project = project_for_search_paths(&mut db, "/project", search_paths);

    let modules = model_modules(&db, project);
    let module_names: Vec<_> = modules
        .iter()
        .map(|module| module.name().as_str())
        .collect();

    assert!(module_names.contains(&"app.models"));
    assert!(module_names.contains(&"somelib.models"));
    assert!(!module_names.contains(&"venv.lib.python3.12.site-packages.somelib.models"));
}

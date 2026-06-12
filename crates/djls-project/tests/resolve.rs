use std::collections::BTreeMap;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_project::*;
use djls_source::Db as _;
use djls_source::FileRootKind;
use djls_source::InMemoryFileSystem;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;

struct TestModel {
    module_path: ModulePath,
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
                    module_path: module.module_path().clone(),
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
fn search_paths_keep_site_packages_external_inside_project_root() {
    let mut fs = InMemoryFileSystem::new();
    fs.add_file("/project/src/app/__init__.py".into(), String::new());
    fs.add_file("/outside/pkg/__init__.py".into(), String::new());
    fs.add_file(
        "/project/.venv/lib/python3.12/site-packages/django/__init__.py".into(),
        String::new(),
    );

    let pythonpath = vec![
        "/project/src".to_string(),
        "/outside".to_string(),
        "/project/.venv/lib/python3.12/site-packages".to_string(),
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

    let pythonpath = vec!["/project/src".to_string()];
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
            .any(|module| module.module_path().as_str() == "blog.models")
    );
    assert!(
        !modules
            .iter()
            .any(|module| module.module_path().as_str() == "src.blog.models")
    );
}

#[test]
fn registering_search_paths_removes_obsolete_external_roots() {
    let db = TestDatabase::new();
    db.add_file("/external/pkg/models.py", "");

    let pythonpath = vec!["/external".to_string()];
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

    let pythonpath = vec!["/shared".to_string()];
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
            .any(|module| module.module_path().as_str() == "blog.models")
    );
}

#[test]
fn templatetag_modules_tolerate_unregistered_search_paths() {
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

    let modules = templatetag_modules(&db, project);
    assert_eq!(modules.len(), 1);
    assert_eq!(
        modules[0].module_path().as_str(),
        "django.templatetags.i18n"
    );
}

#[test]
fn templatetag_resolution_uses_project_venv_site_packages_root() {
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

    let modules = templatetag_modules(&db, project);
    assert_eq!(modules.len(), 1);
    assert_eq!(
        modules[0].module_path().as_str(),
        "django.templatetags.i18n"
    );
    let root = db.files().expect_root(&db, modules[0].path());
    assert_eq!(root.kind(&db), FileRootKind::SearchPath);
}

#[test]
fn templatetag_resolution_prefers_first_party_module_shadowing_dependency() {
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

    let modules = templatetag_modules(&db, project);
    assert_eq!(modules.len(), 1);
    assert_eq!(
        modules[0].path(),
        Utf8Path::new("/project/django/templatetags/i18n.py")
    );
}

#[test]
fn templatetag_modules_preserve_registration_order_across_roots() {
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

    let modules = templatetag_modules(&db, project);
    let module_paths: Vec<_> = modules
        .iter()
        .map(|module| module.module_path().as_str())
        .collect();

    assert_eq!(
        module_paths,
        vec!["a.templatetags.tags", "z.templatetags.tags"]
    );
}

#[test]
fn builtin_registration_modules_preserve_order_across_roots() {
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

    let modules = templatetag_modules(&db, project);
    let module_paths: Vec<_> = modules
        .iter()
        .map(|module| module.module_path().as_str())
        .collect();
    assert_eq!(module_paths, vec!["z_first", "a_second"]);
}

#[test]
fn project_model_graph_refresh_reads_changed_project_file() {
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
    refresh_external_data(&mut db);

    let graph = compute_model_graph(&db, project);
    assert!(graph.get("Article").is_none());
    assert!(graph.get("Comment").is_some());
}

#[test]
fn project_model_discovery_refreshes_through_project_refresh() {
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
    refresh_external_data(&mut db);

    let graph = compute_model_graph(&db, project);
    assert!(graph.get("Article").is_some());
    assert!(graph.get("Comment").is_some());
}

#[test]
fn external_model_graph_refresh_reads_changed_site_packages_file() {
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
    refresh_external_data(&mut db);

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

    let pythonpath = vec!["/zfirst".to_string(), "/afallback".to_string()];
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
    assert_eq!(model.module_path.as_str(), "zapp.models");
}

#[test]
fn external_model_discovery_refreshes_through_project_refresh() {
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
    refresh_external_data(&mut db);

    let graph = compute_model_graph(&db, project);
    assert!(graph.get("Article").is_some());
    assert!(graph.get("Comment").is_some());
}

#[test]
fn external_model_discovery_removes_deleted_models_through_project_refresh() {
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
    refresh_external_data(&mut db);

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

    let pythonpath = vec!["/shared".to_string()];
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
fn refresh_external_data_discovers_site_packages_created_after_bootstrap() {
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

    refresh_external_data(&mut db);

    assert!(project.search_paths(&db).iter().any(|search_path| {
        search_path.path() == Utf8Path::new("/project/.venv/lib/python3.12/site-packages")
    }));
    let graph = compute_model_graph(&db, project);
    assert!(graph.get("VenvArticle").is_some());
}

#[test]
fn discover_external_model_files_finds_models() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

    let app_dir = root.join("myapp");
    std::fs::create_dir_all(&app_dir).unwrap();
    std::fs::write(
        app_dir.join("models.py"),
        r"
from django.db import models

class Article(models.Model):
title = models.CharField(max_length=200)
author = models.ForeignKey('auth.User', on_delete=models.CASCADE)
",
    )
    .unwrap();

    let results = discover_model_files(&djls_source::OsFileSystem, &root, FileRootKind::SearchPath);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0.as_str(), "myapp.models");
    assert!(results[0].1.ends_with("models.py"));
}

#[test]
fn discover_external_model_files_finds_files_without_inspecting_contents() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

    let app_dir = root.join("emptyapp");
    std::fs::create_dir_all(&app_dir).unwrap();
    std::fs::write(app_dir.join("models.py"), "# no models here\n").unwrap();

    // Discovery finds the file (it doesn't inspect contents)
    let results = discover_model_files(&djls_source::OsFileSystem, &root, FileRootKind::SearchPath);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0.as_str(), "emptyapp.models");
}

#[test]
fn discover_external_model_files_nested_apps() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

    for app in &["blog", "accounts"] {
        let app_dir = root.join(app);
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            app_dir.join("models.py"),
            format!(
                "from django.db import models\nclass {name}Model(models.Model):\n    pass\n",
                name = app.chars().next().unwrap().to_uppercase().to_string() + &app[1..]
            ),
        )
        .unwrap();
    }

    let results = discover_model_files(&djls_source::OsFileSystem, &root, FileRootKind::SearchPath);
    assert_eq!(results.len(), 2);
    let module_paths: Vec<&str> = results.iter().map(|(m, _)| m.as_str()).collect();
    assert!(module_paths.contains(&"blog.models"));
    assert!(module_paths.contains(&"accounts.models"));
}

#[test]
fn discover_model_files_workspace_finds_models() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

    let app_dir = root.join("myapp");
    std::fs::create_dir_all(&app_dir).unwrap();
    std::fs::write(
        app_dir.join("models.py"),
        "from django.db import models\nclass Foo(models.Model): pass\n",
    )
    .unwrap();

    let results = discover_model_files(&djls_source::OsFileSystem, &root, FileRootKind::Project);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0.as_str(), "myapp.models");
    assert!(results[0].1.ends_with("models.py"));
}

#[test]
fn discover_external_model_files_package() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

    let models_dir = root.join("myapp/models");
    std::fs::create_dir_all(&models_dir).unwrap();
    std::fs::write(
        models_dir.join("__init__.py"),
        "from .user import User\nfrom .order import Order\n",
    )
    .unwrap();
    std::fs::write(
        models_dir.join("user.py"),
        "from django.db import models\nclass User(models.Model):\n    pass\n",
    )
    .unwrap();
    std::fs::write(
        models_dir.join("order.py"),
        "from django.db import models\nclass Order(models.Model):\n    user = models.ForeignKey(User, on_delete=models.CASCADE)\n",
    )
    .unwrap();

    let results = discover_model_files(&djls_source::OsFileSystem, &root, FileRootKind::SearchPath);
    // Discovers all three files (including __init__.py)
    assert_eq!(results.len(), 3);
    let module_paths: Vec<&str> = results.iter().map(|(m, _)| m.as_str()).collect();
    assert!(module_paths.contains(&"myapp.models"));
    assert!(module_paths.contains(&"myapp.models.user"));
    assert!(module_paths.contains(&"myapp.models.order"));
}

#[test]
fn discover_workspace_models_package() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

    let models_dir = root.join("myapp/models");
    std::fs::create_dir_all(&models_dir).unwrap();
    std::fs::write(models_dir.join("__init__.py"), "").unwrap();
    std::fs::write(
        models_dir.join("user.py"),
        "from django.db import models\nclass User(models.Model): pass\n",
    )
    .unwrap();

    let results = discover_model_files(&djls_source::OsFileSystem, &root, FileRootKind::Project);
    let module_paths: Vec<&str> = results.iter().map(|(m, _)| m.as_str()).collect();
    assert!(
        module_paths.contains(&"myapp.models"),
        "should discover __init__.py as myapp.models"
    );
    assert!(
        module_paths.contains(&"myapp.models.user"),
        "should discover user.py as myapp.models.user"
    );
}

#[test]
fn module_path_from_init_file() {
    let path = Utf8Path::new("myapp/models/__init__.py");
    assert_eq!(
        ModulePath::from_relative_path(path).as_str(),
        "myapp.models"
    );
}

#[test]
fn module_path_from_submodule() {
    let path = Utf8Path::new("myapp/models/user.py");
    assert_eq!(
        ModulePath::from_relative_path(path).as_str(),
        "myapp.models.user"
    );
}

#[test]
fn discover_workspace_models_nested_package() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

    let base_dir = root.join("myapp/models/base");
    std::fs::create_dir_all(&base_dir).unwrap();
    std::fs::write(root.join("myapp/models/__init__.py"), "").unwrap();
    std::fs::write(base_dir.join("__init__.py"), "").unwrap();
    std::fs::write(
        base_dir.join("abstract.py"),
        "from django.db import models\nclass BaseModel(models.Model):\n    class Meta:\n        abstract = True\n",
    )
    .unwrap();

    let results = discover_model_files(&djls_source::OsFileSystem, &root, FileRootKind::Project);
    let module_paths: Vec<&str> = results.iter().map(|(m, _)| m.as_str()).collect();
    assert!(
        module_paths.contains(&"myapp.models.base.abstract"),
        "should discover nested model files: got {module_paths:?}"
    );
}

#[test]
fn discover_external_model_files_nested_package() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

    let base_dir = root.join("myapp/models/base");
    std::fs::create_dir_all(&base_dir).unwrap();
    std::fs::write(root.join("myapp/models/__init__.py"), "").unwrap();
    std::fs::write(base_dir.join("__init__.py"), "").unwrap();
    std::fs::write(
        base_dir.join("abstract.py"),
        "from django.db import models\nclass BaseModel(models.Model):\n    class Meta:\n        abstract = True\n",
    )
    .unwrap();

    let results = discover_model_files(&djls_source::OsFileSystem, &root, FileRootKind::SearchPath);
    let module_paths: Vec<&str> = results.iter().map(|(m, _)| m.as_str()).collect();
    assert!(
        module_paths.contains(&"myapp.models.base.abstract"),
        "should discover nested model files: got {module_paths:?}"
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

    let pythonpath = vec!["/project/venv/lib/python3.12/site-packages".to_string()];
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &Interpreter::InterpreterPath("/usr/bin/python".to_string()),
        &pythonpath,
    );
    search_paths.register_roots(&db);
    let project = project_for_search_paths(&mut db, "/project", search_paths);

    let modules = model_modules(&db, project);
    let module_paths: Vec<_> = modules
        .iter()
        .map(|module| module.module_path().as_str())
        .collect();

    assert!(module_paths.contains(&"app.models"));
    assert!(module_paths.contains(&"somelib.models"));
    assert!(!module_paths.contains(&"venv.lib.python3.12.site-packages.somelib.models"));
}

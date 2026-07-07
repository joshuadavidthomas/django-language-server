use camino::Utf8Path;
use djls_project::PythonModuleName;
use djls_project::testing::django_settings;
use djls_project::testing::extract_model_graph;
use djls_project::testing::filter_arity_status;
use djls_project::testing::model_status;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;

#[test]
fn entry_settings_syntax_error_sets_parse_status_unparseable() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/__init__.py", "")
        .file("/proj/myproject/settings.py", "INSTALLED_APPS = [")
        .install(&mut db);

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();

    assert_eq!(settings["parse_status"], "unparseable");
    assert_eq!(settings["installed_apps"]["values"], serde_json::json!([]));
    assert_eq!(settings["installed_apps"]["extraction"], "partial");
    assert_eq!(settings["templates"]["backends"], serde_json::json!([]));
    assert_eq!(settings["templates"]["extraction"], "partial");
}

#[test]
fn imported_settings_syntax_error_keeps_entry_parse_status_parsed() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings.local")
        .file("/proj/myproject/__init__.py", "")
        .file("/proj/myproject/settings/__init__.py", "")
        .file("/proj/myproject/settings/base.py", "INSTALLED_APPS = [")
        .file(
            "/proj/myproject/settings/local.py",
            "from .base import INSTALLED_APPS, TEMPLATES",
        )
        .install(&mut db);

    let settings = serde_json::to_value(django_settings(&db, project)).unwrap();

    assert_eq!(settings["parse_status"], "parsed");
    assert_eq!(settings["installed_apps"]["values"], serde_json::json!([]));
    assert_eq!(settings["installed_apps"]["extraction"], "partial");
    assert_eq!(settings["templates"]["backends"], serde_json::json!([]));
    assert_eq!(settings["templates"]["extraction"], "partial");
}

#[test]
fn model_parse_failure_is_unparseable_with_empty_graph() {
    let db = TestDatabase::new();
    let path = Utf8Path::new("/proj/blog/models.py");
    db.add_file(path.as_str(), "class Broken(");
    let file = db.file(path);
    let module_name = PythonModuleName::parse("blog.models").unwrap();

    let graph = extract_model_graph(&db, file, module_name.clone());
    let status = serde_json::to_value(model_status(&db, file, module_name)).unwrap();

    assert!(graph.is_empty());
    assert_eq!(status, serde_json::json!("unparseable"));
}

#[test]
fn successful_model_parse_is_partial() {
    let db = TestDatabase::new();
    let path = Utf8Path::new("/proj/blog/models.py");
    db.add_file(
        path.as_str(),
        "from django.db import models\nclass Post(models.Model):\n    pass\n",
    );
    let file = db.file(path);
    let module_name = PythonModuleName::parse("blog.models").unwrap();

    let graph = extract_model_graph(&db, file, module_name.clone());
    let status = serde_json::to_value(model_status(&db, file, module_name)).unwrap();

    assert!(!graph.is_empty());
    assert_eq!(status, serde_json::json!("partial"));
}

#[test]
fn filter_arity_parse_failure_is_unparseable_with_empty_map() {
    let db = TestDatabase::new();
    let path = Utf8Path::new("/proj/blog/templatetags/blog.py");
    db.add_file(path.as_str(), "def broken(");
    let file = db.file(path);
    let module_name = PythonModuleName::parse("blog.templatetags.blog").unwrap();

    let extraction = djls_project::extract_filter_arities(&db, file, module_name.clone());
    let status = serde_json::to_value(filter_arity_status(&db, file, module_name)).unwrap();

    assert!(extraction.arities().is_empty());
    assert_eq!(status, serde_json::json!("unparseable"));
}

#[test]
fn successful_filter_arity_parse_is_partial() {
    let db = TestDatabase::new();
    let path = Utf8Path::new("/proj/blog/templatetags/blog.py");
    db.add_file(
        path.as_str(),
        "from django import template\nregister = template.Library()\n@register.filter\ndef shout(value):\n    return value\n",
    );
    let file = db.file(path);
    let module_name = PythonModuleName::parse("blog.templatetags.blog").unwrap();

    let extraction = djls_project::extract_filter_arities(&db, file, module_name.clone());
    let status = serde_json::to_value(filter_arity_status(&db, file, module_name)).unwrap();

    assert!(!extraction.arities().is_empty());
    assert_eq!(status, serde_json::json!("partial"));
}

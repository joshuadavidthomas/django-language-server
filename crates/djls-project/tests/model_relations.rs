use std::ptr;

use djls_project::ModelGraph;
use djls_project::ModelId;
use djls_project::compute_model_graph;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;

fn model_id<'a>(graph: &'a ModelGraph, name: &'a str, module_name: &str) -> &'a ModelId {
    graph
        .models_named(name)
        .find(|(id, _model)| id.module_name().as_str() == module_name)
        .map(|(id, _model)| id)
        .expect("model should exist")
}

#[test]
fn qualified_relation_resolves_cross_app() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/accounts/models.py",
            "class User(models.Model):\n    pass\n",
        )
        .file(
            "/project/blog/models.py",
            "class User(models.Model):\n    pass\n\nclass Post(models.Model):\n    author = models.ForeignKey(\"accounts.User\", on_delete=models.CASCADE)\n",
        )
        .build(&db);

    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models");
    let accounts_user = model_id(graph, "User", "accounts.models");
    let blog_user = model_id(graph, "User", "blog.models");

    let resolved = graph
        .resolve_relation(post, "author")
        .expect("qualified relation should resolve");
    assert!(ptr::eq(resolved, graph.get_by_id(accounts_user).unwrap()));
    assert!(!ptr::eq(resolved, graph.get_by_id(blog_user).unwrap()));
}

#[test]
fn bare_relation_resolves_relative_to_scope_app() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/accounts/models.py",
            "class User(models.Model):\n    pass\n\nclass Profile(models.Model):\n    user = models.ForeignKey(\"User\", on_delete=models.CASCADE)\n",
        )
        .file(
            "/project/blog/models.py",
            "class User(models.Model):\n    pass\n",
        )
        .build(&db);

    let graph = compute_model_graph(&db, project);
    let profile = model_id(graph, "Profile", "accounts.models");
    let accounts_user = model_id(graph, "User", "accounts.models");
    let blog_user = model_id(graph, "User", "blog.models");

    let resolved = graph
        .resolve_relation(profile, "user")
        .expect("bare relation should resolve in the scope app");
    assert!(ptr::eq(resolved, graph.get_by_id(accounts_user).unwrap()));
    assert!(!ptr::eq(resolved, graph.get_by_id(blog_user).unwrap()));
}

#[test]
fn self_relation_resolves_to_scope_model() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/catalog/models.py",
            "class Category(models.Model):\n    parent = models.ForeignKey(\"self\", on_delete=models.CASCADE)\n",
        )
        .build(&db);

    let graph = compute_model_graph(&db, project);
    let category = model_id(graph, "Category", "catalog.models");

    let resolved = graph
        .resolve_relation(category, "parent")
        .expect("self relation should resolve");
    assert!(ptr::eq(resolved, graph.get_by_id(category).unwrap()));
}

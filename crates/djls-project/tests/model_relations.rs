use std::ptr;

use camino::Utf8Path;
use djls_conf::TagSpecDef;
use djls_project::Interpreter;
use djls_project::ModelGraph;
use djls_project::ModelId;
use djls_project::Project;
use djls_project::SearchPaths;
use djls_project::compute_model_graph;
use djls_source::Db as SourceDb;
use djls_testing::ProjectFixture;
use djls_testing::SalsaEventLog;
use djls_testing::TestDatabase;
use salsa::Database as _;
use serde_json::Value;
use serde_json::json;

fn model_id<'a>(graph: &'a ModelGraph, name: &'a str, module_name: &str) -> &'a ModelId {
    graph
        .models_named(name)
        .find(|(id, _model)| id.module_name().as_str() == module_name)
        .map(|(id, _model)| id)
        .expect("model should exist")
}

fn graph_value(graph: &ModelGraph) -> Value {
    serde_json::to_value(graph).expect("model graph should serialize")
}

fn relation_value<'a>(graph: &'a Value, model: &str, field: &str) -> &'a Value {
    graph["models"][model]["relations"]
        .as_array()
        .and_then(|relations| {
            relations
                .iter()
                .find(|relation| relation["field_name"] == field)
        })
        .expect("relation should exist")
}

fn update_file(db: &mut TestDatabase, path: &str, content: &str) {
    db.add_file(path, content);
    let file = db.get_or_create_file(Utf8Path::new(path));
    db.bump_file_revision(file);
}

fn execution_count(db: &TestDatabase, events: &[salsa::Event], query_name: &str) -> usize {
    events
        .iter()
        .filter(|event| match &event.kind {
            salsa::EventKind::WillExecute { database_key } => db
                .ingredient_debug_name(database_key.ingredient_index())
                .contains(query_name),
            _ => false,
        })
        .count()
}

#[test]
fn qualified_relation_resolves_cross_app() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/accounts/models.py",
            include_str!(
                "testdata/model_relations/qualified_relation_resolves_cross_app/accounts/models.py"
            ),
        )
        .file(
            "/project/blog/models.py",
            include_str!(
                "testdata/model_relations/qualified_relation_resolves_cross_app/blog/models.py"
            ),
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
            include_str!("testdata/model_relations/bare_relation_resolves_relative_to_scope_app/accounts/models.py"),
        )
        .file(
            "/project/blog/models.py",
            include_str!("testdata/model_relations/bare_relation_resolves_relative_to_scope_app/blog/models.py"),
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
            include_str!(
                "testdata/model_relations/self_relation_resolves_to_scope_model/catalog/models.py"
            ),
        )
        .build(&db);

    let graph = compute_model_graph(&db, project);
    let category = model_id(graph, "Category", "catalog.models");

    let resolved = graph
        .resolve_relation(category, "parent")
        .expect("self relation should resolve");
    assert!(ptr::eq(resolved, graph.get_by_id(category).unwrap()));
}

#[test]
fn imported_foreign_key_resolves_to_imported_model_id() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/accounts/models.py",
            include_str!("testdata/model_relations/imported_foreign_key_resolves_to_imported_model_id/accounts/models.py"),
        )
        .file(
            "/project/blog/models.py",
            include_str!("testdata/model_relations/imported_foreign_key_resolves_to_imported_model_id/blog/models.py"),
        )
        .build(&db);

    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models");
    let accounts_user = model_id(graph, "User", "accounts.models");
    let blog_user = model_id(graph, "User", "blog.models");

    let resolved = graph
        .resolve_relation(post, "author")
        .expect("imported relation should resolve");
    assert!(ptr::eq(resolved, graph.get_by_id(accounts_user).unwrap()));
    assert!(!ptr::eq(resolved, graph.get_by_id(blog_user).unwrap()));

    let value = graph_value(graph);
    let relation = relation_value(&value, "blog.models.Post", "author");
    assert_eq!(
        relation["target"],
        json!({ "kind": "Bare", "name": "User" })
    );
    assert_eq!(
        relation["resolution"],
        json!({ "Resolved": "accounts.models.User" })
    );
}

#[test]
fn attribute_qualified_expression_retains_source_path_and_resolves() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/accounts/models.py",
            include_str!("testdata/model_relations/attribute_qualified_expression_retains_source_path_and_resolves/accounts/models.py"),
        )
        .file(
            "/project/blog/models.py",
            include_str!("testdata/model_relations/attribute_qualified_expression_retains_source_path_and_resolves/blog/models.py"),
        )
        .build(&db);

    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models");
    let accounts_user = model_id(graph, "User", "accounts.models");
    let blog_user = model_id(graph, "User", "blog.models");

    let resolved = graph
        .resolve_relation(post, "author")
        .expect("attribute relation should resolve");
    assert!(ptr::eq(resolved, graph.get_by_id(accounts_user).unwrap()));
    assert!(!ptr::eq(resolved, graph.get_by_id(blog_user).unwrap()));

    let value = graph_value(graph);
    let relation = relation_value(&value, "blog.models.Post", "author");
    assert_eq!(
        relation["target"],
        json!({ "kind": "Attribute", "path": ["account_models", "User"] })
    );
    assert_eq!(
        relation["resolution"],
        json!({ "Resolved": "accounts.models.User" })
    );
}

#[test]
fn model_base_import_spellings_are_recognized() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/base_alias/models.py",
            include_str!("testdata/model_relations/model_base_import_spellings_are_recognized/base_alias/models.py"),
        )
        .build(&db);

    let graph = compute_model_graph(&db, project);
    model_id(graph, "FromBase", "base_alias.models");
    model_id(graph, "FromModule", "base_alias.models");
}

#[test]
fn imported_abstract_base_import_spellings_are_recognized_and_inherit_relations() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/a_app/models.py",
            include_str!("testdata/model_relations/imported_abstract_base_import_spellings_are_recognized_and_inherit_relations/a_app/models.py"),
        )
        .file(
            "/project/b_app/models.py",
            include_str!("testdata/model_relations/imported_abstract_base_import_spellings_are_recognized_and_inherit_relations/b_app/models.py"),
        )
        .file(
            "/project/c_app/models.py",
            include_str!("testdata/model_relations/imported_abstract_base_import_spellings_are_recognized_and_inherit_relations/c_app/models.py"),
        )
        .build(&db);

    let graph = compute_model_graph(&db, project);
    let user = model_id(graph, "User", "a_app.models");
    let article = model_id(graph, "Article", "b_app.models");
    let story = model_id(graph, "Story", "c_app.models");

    assert!(ptr::eq(
        graph
            .resolve_relation(article, "owner")
            .expect("direct imported abstract base relation should resolve"),
        graph.get_by_id(user).unwrap()
    ));
    assert!(ptr::eq(
        graph
            .resolve_relation(story, "owner")
            .expect("aliased imported abstract base relation should resolve"),
        graph.get_by_id(user).unwrap()
    ));
}

#[test]
fn dotted_string_auth_user_resolves_via_app_label_path() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/auth/models.py",
            include_str!("testdata/model_relations/dotted_string_auth_user_resolves_via_app_label_path/auth/models.py"),
        )
        .file(
            "/project/shop/models.py",
            include_str!("testdata/model_relations/dotted_string_auth_user_resolves_via_app_label_path/shop/models.py"),
        )
        .build(&db);

    let graph = compute_model_graph(&db, project);
    let order = model_id(graph, "Order", "shop.models");
    let auth_user = model_id(graph, "User", "auth.models");
    let shop_user = model_id(graph, "User", "shop.models");

    let resolved = graph
        .resolve_relation(order, "user")
        .expect("dotted string relation should resolve by app label");
    assert!(ptr::eq(resolved, graph.get_by_id(auth_user).unwrap()));
    assert!(!ptr::eq(resolved, graph.get_by_id(shop_user).unwrap()));

    let value = graph_value(graph);
    let relation = relation_value(&value, "shop.models.Order", "user");
    assert_eq!(
        relation["target"],
        json!({ "kind": "Qualified", "app_label": "auth", "name": "User" })
    );
}

#[test]
fn unresolvable_imported_target_records_explicit_reason() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/blog/models.py",
            include_str!("testdata/model_relations/unresolvable_imported_target_records_explicit_reason/blog/models.py"),
        )
        .build(&db);

    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models");
    assert_eq!(graph.resolve_relation(post, "author"), None);

    let value = graph_value(graph);
    let relation = relation_value(&value, "blog.models.Post", "author");
    assert_eq!(
        relation["resolution"]["Unresolved"]["reason"]["ImportNotFound"],
        json!({ "requested": "missing.models.User" })
    );
}

#[test]
fn imported_module_target_records_explicit_reason() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/accounts/models.py",
            include_str!("testdata/model_relations/imported_module_target_records_explicit_reason/accounts/models.py"),
        )
        .file(
            "/project/blog/models.py",
            include_str!("testdata/model_relations/imported_module_target_records_explicit_reason/blog/models.py"),
        )
        .build(&db);

    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models");
    assert_eq!(graph.resolve_relation(post, "author"), None);

    let value = graph_value(graph);
    let relation = relation_value(&value, "blog.models.Post", "author");
    assert_eq!(
        relation["resolution"]["Unresolved"]["reason"]["ImportedTargetIsModule"],
        json!({ "module": "accounts.models" })
    );
}

#[test]
fn imported_partial_target_is_preserved_when_model_id_is_absent() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/accounts/models.py",
            include_str!("testdata/model_relations/imported_partial_target_is_preserved_when_model_id_is_absent/accounts/models.py"),
        )
        .file(
            "/project/blog/models.py",
            include_str!("testdata/model_relations/imported_partial_target_is_preserved_when_model_id_is_absent/blog/models.py"),
        )
        .build(&db);

    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models");
    assert_eq!(graph.resolve_relation(post, "author"), None);

    let value = graph_value(graph);
    let relation = relation_value(&value, "blog.models.Post", "author");
    assert_eq!(
        relation["resolution"],
        json!({
            "Partial": {
                "resolved_prefix": "accounts.models",
                "unresolved_tail": ["User"]
            }
        })
    );
}

#[test]
fn ambiguous_app_label_fallback_is_preserved_in_relation_resolution() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/News/models.py",
            include_str!("testdata/model_relations/ambiguous_app_label_fallback_is_preserved_in_relation_resolution/news_title/models.py"),
        )
        .file(
            "/project/news/models.py",
            include_str!("testdata/model_relations/ambiguous_app_label_fallback_is_preserved_in_relation_resolution/news_lower/models.py"),
        )
        .file(
            "/project/NEWS/models.py",
            include_str!("testdata/model_relations/ambiguous_app_label_fallback_is_preserved_in_relation_resolution/news_upper/models.py"),
        )
        .build(&db);

    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "NEWS.models");
    assert_eq!(graph.resolve_relation(post, "author"), None);

    let value = graph_value(graph);
    let relation = relation_value(&value, "NEWS.models.Post", "author");
    assert_eq!(
        relation["resolution"]["Ambiguous"]["candidates"],
        json!(["News.models.User", "news.models.User"])
    );
    assert_eq!(
        relation["resolution"]["Ambiguous"]["app_label"],
        json!("NEWS")
    );
    assert_eq!(relation["resolution"]["Ambiguous"]["name"], json!("User"));
}

#[test]
fn computed_model_graph_does_not_expose_file_local_relation_resolution() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/blog/models.py",
            include_str!("testdata/model_relations/computed_model_graph_does_not_expose_file_local_relation_resolution/blog/models.py"),
        )
        .build(&db);

    let graph = compute_model_graph(&db, project);
    let value = graph_value(graph);
    assert_eq!(
        relation_value(&value, "blog.models.TaggedItem", "target")["resolution"],
        json!({
            "Unresolved": {
                "reason": {
                    "SameAppTargetNotFound": {
                        "app_label": "blog",
                        "name": "Missing"
                    }
                }
            }
        })
    );
    assert_eq!(
        relation_value(&value, "blog.models.TaggedItem", "content_object")["resolution"],
        json!({ "Unresolved": { "reason": "NoStaticTarget" } })
    );
}

#[test]
fn salsa_recomputes_relation_resolution_for_import_edits_only_where_needed() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    db.add_file(
        "/project/accounts/models.py",
        include_str!("testdata/model_relations/salsa_recomputes_relation_resolution_for_import_edits_only_where_needed/accounts/models.py"),
    );
    db.add_file(
        "/project/blog/models.py",
        include_str!("testdata/model_relations/salsa_recomputes_relation_resolution_for_import_edits_only_where_needed/blog/models_initial.py"),
    );
    db.add_file(
        "/project/other/models.py",
        include_str!("testdata/model_relations/salsa_recomputes_relation_resolution_for_import_edits_only_where_needed/other/models_initial.py"),
    );

    let interpreter = Interpreter::Auto;
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &interpreter,
        &[],
    );
    search_paths.register_roots(&db);
    let project = Project::new(
        &db,
        Utf8Path::new("/project").to_path_buf(),
        search_paths,
        interpreter,
        None,
        Vec::new(),
        Vec::new(),
        TagSpecDef::default(),
    );
    db.set_project(project);

    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models");
    let user = model_id(graph, "User", "accounts.models");
    assert!(ptr::eq(
        graph
            .resolve_relation(post, "author")
            .expect("initial import should resolve"),
        graph.get_by_id(user).unwrap()
    ));
    let _ = event_log.take();

    update_file(
        &mut db,
        "/project/blog/models.py",
        include_str!(
            "testdata/model_relations/salsa_recomputes_relation_resolution_for_import_edits_only_where_needed/blog/models_updated.py"
        ),
    );
    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models");
    let profile = model_id(graph, "Profile", "accounts.models");
    assert!(ptr::eq(
        graph
            .resolve_relation(post, "author")
            .expect("edited import should resolve"),
        graph.get_by_id(profile).unwrap()
    ));
    assert!(execution_count(&db, &event_log.take(), "compute_model_graph") > 0);

    update_file(
        &mut db,
        "/project/other/models.py",
        include_str!(
            "testdata/model_relations/salsa_recomputes_relation_resolution_for_import_edits_only_where_needed/other/models_updated.py"
        ),
    );
    let _graph = compute_model_graph(&db, project);
    let events = event_log.take();
    assert_eq!(execution_count(&db, &events, "extract_models"), 1);
}

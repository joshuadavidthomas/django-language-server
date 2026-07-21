use std::ptr;

use camino::Utf8Path;
use djls_conf::TagSpecDef;
use djls_project::Interpreter;
use djls_project::ModelGraph;
use djls_project::ModelId;
use djls_project::Project;
use djls_project::PythonModuleName;
use djls_project::SearchPaths;
use djls_project::compute_model_graph;
use djls_project::testing::PythonSyntaxErrorClass;
use djls_project::testing::extract_model_graph;
use djls_project::testing::model_location;
use djls_project::testing::model_relation_locations;
use djls_project::testing::python_syntax_errors;
use djls_source::ChangeEvent;
use djls_source::Db as _;
use djls_source::SourceChanges;
use djls_source::Span;
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
                .find(|relation| relation["field_name"]["value"] == field)
        })
        .expect("relation should exist")
}

fn update_file(db: &mut TestDatabase, path: &str, content: &str) {
    db.add_file(path, content);
    SourceChanges::new([ChangeEvent::ContentChanged(path.into())]).apply(db);
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

fn expected_span(source: &str, needle: &str) -> Span {
    let start = source
        .find(needle)
        .unwrap_or_else(|| panic!("expected source to contain {needle:?}"));
    Span::saturating_from_parts_usize(start, needle.len())
}

fn expected_span_after(source: &str, anchor: &str, needle: &str) -> Span {
    let anchor_start = source
        .find(anchor)
        .unwrap_or_else(|| panic!("expected source to contain anchor {anchor:?}"));
    let relative_start = source[anchor_start..]
        .find(needle)
        .unwrap_or_else(|| panic!("expected {needle:?} after {anchor:?}"));
    Span::saturating_from_parts_usize(anchor_start + relative_start, needle.len())
}

fn assert_relation_location(
    actual: &(String, djls_source::File, Span, Option<Span>),
    field: &str,
    file: djls_source::File,
    field_span: Span,
    target_span: Option<Span>,
) {
    assert_eq!(actual.0, field);
    assert_eq!(actual.1, file);
    assert_eq!(actual.2, field_span);
    assert_eq!(actual.3, target_span);
}

#[salsa::tracked(returns(copy))]
#[allow(clippy::needless_pass_by_value)]
fn model_graph_span_probe(
    db: &dyn djls_project::Db,
    project: Project,
    module_name: String,
    model_name: String,
) -> u32 {
    let graph = compute_model_graph(db, project);
    let mut checksum = u32::try_from(graph.len()).expect("test model graph should fit in u32");

    if let Some((_file, span)) = model_location(graph, module_name.as_str(), model_name.as_str()) {
        checksum = checksum
            .wrapping_add(span.start())
            .wrapping_add(span.length());
    }

    for (_field_name, _file, field_span, target_span) in
        model_relation_locations(graph, module_name.as_str(), model_name.as_str())
    {
        checksum = checksum
            .wrapping_add(field_span.start())
            .wrapping_add(field_span.length());
        if let Some(target_span) = target_span {
            checksum = checksum
                .wrapping_add(target_span.start())
                .wrapping_add(target_span.length());
        }
    }

    checksum
}

fn probe_model(db: &TestDatabase, project: Project, module_name: &str, model_name: &str) -> u32 {
    model_graph_span_probe(db, project, module_name.to_string(), model_name.to_string())
}

#[test]
fn recovered_syntax_retains_imports_and_model_facts_with_error_span() {
    let source =
        "from django.db import models\n\nclass Post(models.Model):\n    pass\n\ndef broken(";
    let db = TestDatabase::new();
    let path = Utf8Path::new("/project/blog/models.py");
    db.add_file(path.as_str(), source);
    let file = db.file(path);
    let module_name = PythonModuleName::parse("blog.models").unwrap();

    let graph = extract_model_graph(&db, file, module_name);
    assert!(
        graph
            .models_named("Post")
            .any(|(id, _)| id.module_name().as_str() == "blog.models")
    );

    let errors = python_syntax_errors(&db, file).expect("file should be Python");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].class, PythonSyntaxErrorClass::Ordinary);
    assert_eq!(
        errors[0].span,
        Span::new(u32::try_from(source.len()).unwrap(), 0)
    );
}

#[test]
fn model_graph_span_probe_reexecutes_when_model_span_shifts() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let initial = include_str!(
        "testdata/model_relations/model_graph_span_probe_reexecutes_when_model_span_shifts/accounts/models_initial.py"
    );
    let project = ProjectFixture::new("/project")
        .file("/project/accounts/models.py", initial)
        .build(&db);

    let before = probe_model(&db, project, "accounts.models", "User");
    let _ = event_log.take();

    update_file(
        &mut db,
        "/project/accounts/models.py",
        include_str!(
            "testdata/model_relations/model_graph_span_probe_reexecutes_when_model_span_shifts/accounts/models_updated.py"
        ),
    );
    let after = probe_model(&db, project, "accounts.models", "User");
    let events = event_log.take();

    assert_ne!(after, before);
    assert!(execution_count(&db, &events, "model_graph_span_probe") > 0);
}

#[test]
fn model_graph_span_probe_reexecutes_when_relation_is_added() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let project = ProjectFixture::new("/project")
        .file(
            "/project/accounts/models.py",
            include_str!(
                "testdata/model_relations/model_graph_span_probe_reexecutes_when_relation_is_added/accounts/models.py"
            ),
        )
        .file(
            "/project/blog/models.py",
            include_str!(
                "testdata/model_relations/model_graph_span_probe_reexecutes_when_relation_is_added/blog/models_initial.py"
            ),
        )
        .build(&db);

    let before = probe_model(&db, project, "blog.models", "Post");
    let _ = event_log.take();

    update_file(
        &mut db,
        "/project/blog/models.py",
        include_str!(
            "testdata/model_relations/model_graph_span_probe_reexecutes_when_relation_is_added/blog/models_updated.py"
        ),
    );
    let after = probe_model(&db, project, "blog.models", "Post");
    let events = event_log.take();

    assert_ne!(after, before);
    assert!(execution_count(&db, &events, "model_graph_span_probe") > 0);
}

#[test]
fn model_graph_span_probe_backdates_for_trailing_whitespace() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let project = ProjectFixture::new("/project")
        .file(
            "/project/accounts/models.py",
            include_str!(
                "testdata/model_relations/model_graph_span_probe_backdates_for_trailing_whitespace/accounts/models_initial.py"
            ),
        )
        .build(&db);

    let before = probe_model(&db, project, "accounts.models", "User");
    let _ = event_log.take();

    update_file(
        &mut db,
        "/project/accounts/models.py",
        concat!(
            include_str!(
                "testdata/model_relations/model_graph_span_probe_backdates_for_trailing_whitespace/accounts/models_initial.py"
            ),
            "   \n"
        ),
    );
    let after = probe_model(&db, project, "accounts.models", "User");
    let events = event_log.take();

    assert_eq!(after, before);
    assert_eq!(execution_count(&db, &events, "model_graph_span_probe"), 0);
}

#[test]
fn model_graph_span_probe_backdates_for_span_shift_in_file_without_model_facts() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let project = ProjectFixture::new("/project")
        .file(
            "/project/empty/models.py",
            include_str!(
                "testdata/model_relations/model_graph_span_probe_backdates_for_span_shift_in_file_without_model_facts/empty/models_initial.py"
            ),
        )
        .build(&db);

    let before = probe_model(&db, project, "empty.models", "Missing");
    let _ = event_log.take();

    update_file(
        &mut db,
        "/project/empty/models.py",
        include_str!(
            "testdata/model_relations/model_graph_span_probe_backdates_for_span_shift_in_file_without_model_facts/empty/models_updated.py"
        ),
    );
    let after = probe_model(&db, project, "empty.models", "Missing");
    let events = event_log.take();

    assert_eq!(after, before);
    assert_eq!(execution_count(&db, &events, "model_graph_span_probe"), 0);
}

#[test]
fn model_graph_span_probe_reexecutes_when_deferred_child_inherits_shifted_relation_span() {
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let base_initial = include_str!(
        "testdata/model_relations/model_graph_span_probe_reexecutes_when_deferred_child_inherits_shifted_relation_span/base/models_initial.py"
    );
    let project = ProjectFixture::new("/project")
        .file("/project/base/models.py", base_initial)
        .file(
            "/project/blog/models.py",
            include_str!(
                "testdata/model_relations/model_graph_span_probe_reexecutes_when_deferred_child_inherits_shifted_relation_span/blog/models.py"
            ),
        )
        .build(&db);

    let before = probe_model(&db, project, "blog.models", "Article");
    let _ = event_log.take();

    update_file(
        &mut db,
        "/project/base/models.py",
        include_str!(
            "testdata/model_relations/model_graph_span_probe_reexecutes_when_deferred_child_inherits_shifted_relation_span/base/models_updated.py"
        ),
    );
    let after = probe_model(&db, project, "blog.models", "Article");
    let events = event_log.take();

    assert_ne!(after, before);
    assert!(execution_count(&db, &events, "model_graph_span_probe") > 0);
}

#[test]
fn model_graph_records_model_and_relation_provenance_for_relation_forms() {
    let db = TestDatabase::new();
    let source = include_str!(
        "testdata/model_relations/model_graph_records_model_and_relation_provenance_for_relation_forms/blog/models.py"
    );
    let project = ProjectFixture::new("/project")
        .file(
            "/project/accounts/models.py",
            include_str!(
                "testdata/model_relations/model_graph_records_model_and_relation_provenance_for_relation_forms/accounts/models.py"
            ),
        )
        .file("/project/blog/models.py", source)
        .build(&db);

    let graph = compute_model_graph(&db, project);
    let blog_file = db.file(Utf8Path::new("/project/blog/models.py"));
    let (model_file, model_span) =
        model_location(graph, "blog.models", "Post").expect("Post model should have location");
    assert_eq!(model_file, blog_file);
    assert_eq!(model_span, expected_span(source, "Post"));

    let locations = model_relation_locations(graph, "blog.models", "Post");
    let location = |field: &str| {
        locations
            .iter()
            .find(|(name, ..)| name == field)
            .unwrap_or_else(|| panic!("expected relation location for {field}"))
    };

    assert_relation_location(
        location("author"),
        "author",
        blog_file,
        expected_span(source, "author"),
        Some(expected_span(source, "\"accounts.User\"")),
    );
    assert_relation_location(
        location("editor"),
        "editor",
        blog_file,
        expected_span(source, "editor"),
        Some(expected_span(source, "account_models.User")),
    );
    assert_relation_location(
        location("parent"),
        "parent",
        blog_file,
        expected_span(source, "parent"),
        Some(expected_span(source, "\"self\"")),
    );
    assert_relation_location(
        location("tags"),
        "tags",
        blog_file,
        expected_span(source, "tags"),
        Some(expected_span(source, "\"Tag\"")),
    );
    assert_relation_location(
        location("content_object"),
        "content_object",
        blog_file,
        expected_span(source, "content_object"),
        None,
    );
}

#[test]
fn model_graph_records_inherited_relation_provenance() {
    let db = TestDatabase::new();
    let same_file_source = include_str!(
        "testdata/model_relations/model_graph_records_inherited_relation_provenance/inheritance/models.py"
    );
    let base_source = include_str!(
        "testdata/model_relations/model_graph_records_inherited_relation_provenance/base/models.py"
    );
    let child_source = include_str!(
        "testdata/model_relations/model_graph_records_inherited_relation_provenance/child/models.py"
    );
    let project = ProjectFixture::new("/project")
        .file("/project/inheritance/models.py", same_file_source)
        .file("/project/base/models.py", base_source)
        .file("/project/child/models.py", child_source)
        .build(&db);

    let graph = compute_model_graph(&db, project);
    let same_file = db.file(Utf8Path::new("/project/inheritance/models.py"));
    let base_file = db.file(Utf8Path::new("/project/base/models.py"));

    for child in ["SameFileChild", "SameFileSibling"] {
        let locations = model_relation_locations(graph, "inheritance.models", child);
        let location = |field: &str| {
            locations
                .iter()
                .find(|(name, ..)| name == field)
                .unwrap_or_else(|| panic!("expected relation location for {child}.{field}"))
        };

        assert_relation_location(
            location("grand_owner"),
            "grand_owner",
            same_file,
            expected_span(same_file_source, "grand_owner"),
            Some(expected_span(
                same_file_source,
                "\"inheritance.GrandTarget\"",
            )),
        );
        assert_relation_location(
            location("parent_owner"),
            "parent_owner",
            same_file,
            expected_span(same_file_source, "parent_owner"),
            Some(expected_span(
                same_file_source,
                "\"inheritance.ParentTarget\"",
            )),
        );
    }

    for child in ["CrossChild", "CrossSibling"] {
        let locations = model_relation_locations(graph, "child.models", child);
        let location = |field: &str| {
            locations
                .iter()
                .find(|(name, ..)| name == field)
                .unwrap_or_else(|| panic!("expected relation location for {child}.{field}"))
        };

        assert_relation_location(
            location("base_owner"),
            "base_owner",
            base_file,
            expected_span(base_source, "base_owner"),
            Some(expected_span(base_source, "\"base.BaseTarget\"")),
        );
    }
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
fn later_same_name_class_shadows_imported_base_for_relation() {
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

    // Occurrence-local resolution follows Python scoping: the later
    // `class User` rebinds the name imported from `accounts.models`, so the FK
    // occurrence resolves to the local `blog.models.User`, not the import.
    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models");
    let accounts_user = model_id(graph, "User", "accounts.models");
    let blog_user = model_id(graph, "User", "blog.models");

    let resolved = graph
        .resolve_relation(post, "author")
        .expect("shadowed relation should resolve to the local class");
    assert!(ptr::eq(resolved, graph.get_by_id(blog_user).unwrap()));
    assert!(!ptr::eq(resolved, graph.get_by_id(accounts_user).unwrap()));

    let value = graph_value(graph);
    let relation = relation_value(&value, "blog.models.Post", "author");
    assert_eq!(
        relation["target"]["value"],
        json!({ "kind": "Bare", "name": "User" })
    );
    assert_eq!(
        relation["resolution"],
        json!({ "Resolved": "blog.models.User" })
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
        relation["target"]["value"],
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
        relation["target"]["value"],
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

/// Detect whether occurrence-local model extraction recognizes `class` as a
/// Django model in a single-file module. Exercises the source-order alias
/// scanner end to end through the `extract_models` query.
fn detects_model(source: &str, class: &str) -> bool {
    let db = TestDatabase::new();
    let path = Utf8Path::new("/project/app/models.py");
    db.add_file(path.as_str(), source);
    let file = db.file(path);
    let module_name = PythonModuleName::parse("app.models").unwrap();
    extract_model_graph(&db, file, module_name)
        .models_named(class)
        .next()
        .is_some()
}

#[test]
fn control_import_before_class_is_recognized() {
    assert!(detects_model(
        "from django.db import models\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
}

#[test]
fn control_recognized_base_import_spellings() {
    // Direct alias, dotted unaliased, and named from-import alias all remain
    // recognized under occurrence-local resolution.
    assert!(detects_model(
        "import django.db.models as dm\nclass Thing(dm.Model):\n    pass\n",
        "Thing",
    ));
    assert!(detects_model(
        "import django.db.models\nclass Thing(django.db.models.Model):\n    pass\n",
        "Thing",
    ));
    assert!(detects_model(
        "from django.db.models import Model as Base\nclass Thing(Base):\n    pass\n",
        "Thing",
    ));
}

#[test]
fn control_django_db_models_recognized_without_external_source() {
    // No accounts/django sources are provided; recognition is purely symbolic.
    assert!(detects_model(
        "from django.db import models\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
}

#[test]
fn correction_import_after_class_does_not_affect_earlier_class() {
    assert!(!detects_model(
        "class Thing(models.Model):\n    pass\nfrom django.db import models\n",
        "Thing",
    ));
}

#[test]
fn correction_reassignment_before_occurrence_removes_certainty() {
    assert!(!detects_model(
        "from django.db import models\nmodels = None\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
}

#[test]
fn correction_delete_before_occurrence_removes_certainty() {
    assert!(!detects_model(
        "from django.db import models\ndel models\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
}

#[test]
fn correction_function_definition_before_occurrence_removes_certainty() {
    assert!(!detects_model(
        "from django.db import models\ndef models():\n    pass\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
}

#[test]
fn correction_class_definition_before_occurrence_removes_certainty() {
    assert!(!detects_model(
        "from django.db import models\nclass models:\n    pass\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
}

#[test]
fn correction_branch_disagreement_is_conservatively_unresolved() {
    assert!(!detects_model(
        "from django.db import models\nif flag:\n    models = None\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
}

#[test]
fn correction_try_disagreement_is_conservatively_unresolved() {
    assert!(!detects_model(
        "from django.db import models\ntry:\n    models = None\nexcept Exception:\n    pass\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
}

#[test]
fn correction_later_exact_import_restores_certainty_for_later_only() {
    let source = concat!(
        "from django.db import models\n",
        "models = None\n",
        "class Early(models.Model):\n    pass\n",
        "from django.db import models\n",
        "class Late(models.Model):\n    pass\n",
    );
    assert!(!detects_model(source, "Early"));
    assert!(detects_model(source, "Late"));
}

#[test]
fn correction_earlier_occurrence_stable_after_later_write() {
    let source = concat!(
        "from django.db import models\n",
        "class Early(models.Model):\n    pass\n",
        "models = None\n",
        "class Late(models.Model):\n    pass\n",
    );
    assert!(detects_model(source, "Early"));
    assert!(!detects_model(source, "Late"));
}

#[test]
fn contained_model_occurrences_use_branch_local_aliases_without_exporting_them() {
    let source = concat!(
        "if enabled:\n",
        "    from django.db import models\n",
        "    class Inside(models.Model):\n",
        "        pass\n",
        "class Outside(models.Model):\n",
        "    pass\n",
    );
    assert!(detects_model(source, "Inside"));
    assert!(!detects_model(source, "Outside"));
}

#[test]
fn compound_binding_targets_invalidate_aliases_inside_their_bodies() {
    assert!(!detects_model(
        "from django.db import models\nfor models in values:\n    class Thing(models.Model):\n        pass\n",
        "Thing",
    ));
    assert!(!detects_model(
        "from django.db import models\nmatch value:\n    case models:\n        class Thing(models.Model):\n            pass\n",
        "Thing",
    ));
    assert!(!detects_model(
        "from django.db import models\ntry:\n    pass\nexcept Exception as models:\n    class Thing(models.Model):\n        pass\n",
        "Thing",
    ));
    assert!(!detects_model(
        "from django.db import models\nwith resource() as models:\n    class Thing(models.Model):\n        pass\n",
        "Thing",
    ));
}

#[test]
fn unusable_exact_relative_import_removes_stale_symbolic_alias() {
    assert!(!detects_model(
        "from django.db import models\nfrom ....missing import models\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
}

#[test]
fn relative_model_alias_resolves_an_imported_abstract_base() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/app/base/models.py",
            "from django.db import models\nclass AbstractThing(models.Model):\n    class Meta:\n        abstract = True\n",
        )
        .file(
            "/project/app/models.py",
            "from .base.models import AbstractThing as Base\nclass Thing(Base):\n    pass\n",
        )
        .build(&db);

    let graph = compute_model_graph(&db, project);
    model_id(graph, "Thing", "app.models");
}

#[test]
fn class_body_relations_capture_alias_state_at_each_occurrence() {
    let source = concat!(
        "from django.db import models\n",
        "class Post(models.Model):\n",
        "    before = models.ForeignKey(User, on_delete=models.CASCADE)\n",
        "    from accounts.models import User\n",
        "    imported = models.ForeignKey(User, on_delete=models.CASCADE)\n",
        "    User = object()\n",
        "    after = models.ForeignKey(User, on_delete=models.CASCADE)\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/accounts/models.py",
            "from django.db import models\nclass User(models.Model):\n    pass\n",
        )
        .file("/project/blog/models.py", source)
        .build(&db);

    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models");
    let accounts_user = model_id(graph, "User", "accounts.models");
    assert_eq!(graph.resolve_relation(post, "before"), None);
    assert!(ptr::eq(
        graph
            .resolve_relation(post, "imported")
            .expect("class-local import should resolve at its occurrence"),
        graph.get_by_id(accounts_user).unwrap(),
    ));
    assert_eq!(graph.resolve_relation(post, "after"), None);

    let value = graph_value(graph);
    assert_eq!(
        relation_value(&value, "blog.models.Post", "imported")["resolution"],
        json!({ "Resolved": "accounts.models.User" })
    );
    assert_eq!(
        relation_value(&value, "blog.models.Post", "before")["resolution"]["Unresolved"]["reason"]
            ["SameAppTargetNotFound"]["name"],
        json!("User")
    );
    assert_eq!(
        relation_value(&value, "blog.models.Post", "after")["resolution"]["Unresolved"]["reason"]["MissingImportBinding"],
        json!({ "binding": "User" })
    );

    let file = db.file(Utf8Path::new("/project/blog/models.py"));
    let locations = model_relation_locations(graph, "blog.models", "Post");
    for (field, anchor) in [
        ("before", "before ="),
        ("imported", "imported ="),
        ("after", "after ="),
    ] {
        let location = locations
            .iter()
            .find(|location| location.0 == field)
            .expect("relation location should exist");
        assert_relation_location(
            location,
            field,
            file,
            expected_span_after(source, anchor, field),
            Some(expected_span_after(source, anchor, "User")),
        );
    }
}

#[test]
fn class_body_compound_aliases_apply_only_to_contained_relations() {
    let source = concat!(
        "from django.db import models\n",
        "class Post(models.Model):\n",
        "    if enabled:\n",
        "        from accounts.models import User\n",
        "        conditional = models.ForeignKey(User, on_delete=models.CASCADE)\n",
        "    outside = models.ForeignKey(User, on_delete=models.CASCADE)\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/accounts/models.py",
            "from django.db import models\nclass User(models.Model):\n    pass\n",
        )
        .file("/project/blog/models.py", source)
        .build(&db);

    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models");
    assert!(graph.resolve_relation(post, "conditional").is_some());
    assert_eq!(graph.resolve_relation(post, "outside"), None);
}

#[test]
fn scanner_visits_every_compound_body_and_preserves_untouched_aliases() {
    let source = concat!(
        "from django.db import models\n",
        "for item in values:\n    class InFor(models.Model):\n        pass\n",
        "while enabled:\n    class InWhile(models.Model):\n        pass\n",
        "try:\n    class InTry(models.Model):\n        pass\n",
        "except Exception:\n    class InExcept(models.Model):\n        pass\n",
        "match value:\n    case _:\n        class InMatch(models.Model):\n            pass\n",
        "with resource():\n    class InWith(models.Model):\n        pass\n",
        "if enabled:\n    unrelated = None\n",
        "class After(models.Model):\n    pass\n",
    );

    for name in [
        "InFor", "InWhile", "InTry", "InExcept", "InMatch", "InWith", "After",
    ] {
        assert!(detects_model(source, name), "{name} should be recognized");
    }
}

#[test]
fn all_exact_write_forms_invalidate_symbolic_aliases() {
    assert!(!detects_model(
        "from django.db import models\nmodels: object = None\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
    assert!(!detects_model(
        "from django.db import models\nmodels += other\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
    assert!(!detects_model(
        "from django.db import models\ntype models = object\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
}

#[test]
fn star_import_preserves_known_model_alias_and_function_body_is_isolated() {
    let source = concat!(
        "from django.db import models\n",
        "from unknown import *\n",
        "def helper():\n",
        "    models = None\n",
        "    class Inner(models.Model):\n",
        "        pass\n",
        "class Outer(models.Model):\n",
        "    pass\n",
    );
    assert!(!detects_model(source, "Inner"));
    assert!(detects_model(source, "Outer"));
}

#[test]
fn deferred_imported_base_is_not_rebound_by_a_later_local_class() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/accounts/models.py",
            concat!(
                "from django.db import models\n",
                "class AbstractBase(models.Model):\n",
                "    class Meta:\n",
                "        abstract = True\n",
            ),
        )
        .file(
            "/project/blog/models.py",
            concat!(
                "from accounts.models import AbstractBase as Base\n",
                "class Child(Base):\n",
                "    pass\n",
                "class Base:\n",
                "    pass\n",
            ),
        )
        .build(&db);

    let graph = compute_model_graph(&db, project);
    model_id(graph, "Child", "blog.models");
}

#[test]
fn loop_and_match_writes_invalidate_aliases_after_the_compound() {
    assert!(!detects_model(
        "from django.db import models\nfor item in values:\n    models = None\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
    assert!(!detects_model(
        "from django.db import models\nmatch value:\n    case _:\n        models = None\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
}

#[test]
fn string_relation_targets_ignore_python_import_aliases() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/accounts/models.py",
            "from django.db import models\nclass User(models.Model):\n    pass\n",
        )
        .file(
            "/project/blog/models.py",
            concat!(
                "from django.db import models\n",
                "from accounts.models import User\n",
                "class User(models.Model):\n    pass\n",
                "class Post(models.Model):\n",
                "    author = models.ForeignKey(\"User\", on_delete=models.CASCADE)\n",
            ),
        )
        .build(&db);

    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models");
    let blog_user = model_id(graph, "User", "blog.models");
    assert!(ptr::eq(
        graph
            .resolve_relation(post, "author")
            .expect("string target should resolve in the current app"),
        graph.get_by_id(blog_user).unwrap(),
    ));
}

#[test]
fn explicitly_shadowed_relation_does_not_fall_back_to_same_app_model() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/blog/models.py",
            concat!(
                "from django.db import models\n",
                "class User(models.Model):\n    pass\n",
                "User = object()\n",
                "class Post(models.Model):\n",
                "    author = models.ForeignKey(User, on_delete=models.CASCADE)\n",
            ),
        )
        .build(&db);

    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models");
    assert_eq!(graph.resolve_relation(post, "author"), None);
    let value = graph_value(graph);
    assert_eq!(
        relation_value(&value, "blog.models.Post", "author")["resolution"]["Unresolved"]["reason"]
            ["MissingImportBinding"],
        json!({ "binding": "User" })
    );
}

#[test]
fn unresolved_qualified_base_does_not_collapse_to_local_class_name() {
    let source = concat!(
        "from django.db import models\n",
        "class AbstractBase(models.Model):\n",
        "    class Meta:\n        abstract = True\n",
        "class Child(missing.AbstractBase):\n",
        "    pass\n",
    );
    assert!(!detects_model(source, "Child"));
}

#[test]
fn earlier_deferred_model_cannot_overwrite_a_later_direct_definition() {
    let source = concat!(
        "from django.db import models\n",
        "from accounts.models import AbstractBase as Base\n",
        "class Thing(Base):\n",
        "    old = models.ForeignKey(\"self\", on_delete=models.CASCADE)\n",
        "class Thing(models.Model):\n",
        "    new = models.ForeignKey(\"self\", on_delete=models.CASCADE)\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/accounts/models.py",
            concat!(
                "from django.db import models\n",
                "class AbstractBase(models.Model):\n",
                "    class Meta:\n        abstract = True\n",
            ),
        )
        .file("/project/blog/models.py", source)
        .build(&db);

    let graph = compute_model_graph(&db, project);
    let thing = model_id(graph, "Thing", "blog.models");
    assert_eq!(graph.resolve_relation(thing, "old"), None);
    assert!(graph.resolve_relation(thing, "new").is_some());
    let (_file, span) = model_location(graph, "blog.models", "Thing").unwrap();
    assert_eq!(
        span,
        expected_span_after(source, "class Thing(models.Model)", "Thing")
    );
}

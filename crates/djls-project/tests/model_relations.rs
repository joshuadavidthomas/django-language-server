use std::io;
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

fn model_id<'a>(
    graph: &'a ModelGraph,
    name: &'a str,
    module_name: &str,
) -> Result<&'a ModelId, io::Error> {
    graph
        .models_named(name)
        .find(|(id, _model)| id.module_name().as_str() == module_name)
        .map(|(id, _model)| id)
        .ok_or_else(|| io::Error::other(format!("model `{module_name}.{name}` does not exist")))
}

fn graph_value(graph: &ModelGraph) -> Result<Value, serde_json::Error> {
    serde_json::to_value(graph)
}

fn relation_value<'a>(graph: &'a Value, model: &str, field: &str) -> Result<&'a Value, io::Error> {
    graph["models"][model]["relations"]
        .as_array()
        .and_then(|relations| {
            relations
                .iter()
                .find(|relation| relation["field_name"]["value"] == field)
        })
        .ok_or_else(|| io::Error::other(format!("relation `{model}.{field}` does not exist")))
}

fn update_file(
    db: &mut TestDatabase,
    path: &str,
    content: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    db.add_file(path, content)?;
    SourceChanges::new([ChangeEvent::ContentChanged(path.into())]).apply(db);
    Ok(())
}

fn execution_count(db: &TestDatabase, events: &[salsa::Event], query_name: &str) -> usize {
    events
        .iter()
        .filter(|event| match &event.kind {
            salsa::EventKind::WillExecute { database_key } => db
                .ingredient_debug_name(database_key.ingredient_index())
                .contains(query_name),
            salsa::EventKind::DidValidateMemoizedValue { .. }
            | salsa::EventKind::WillBlockOn { .. }
            | salsa::EventKind::WillIterateCycle { .. }
            | salsa::EventKind::DidFinalizeCycle { .. }
            | salsa::EventKind::WillCheckCancellation
            | salsa::EventKind::DidSetCancellationFlag
            | salsa::EventKind::WillDiscardStaleOutput { .. }
            | salsa::EventKind::DidDiscard { .. }
            | salsa::EventKind::DidDiscardAccumulated { .. }
            | salsa::EventKind::DidInternValue { .. }
            | salsa::EventKind::DidReuseInternedValue { .. }
            | salsa::EventKind::DidValidateInternedValue { .. } => false,
        })
        .count()
}

type RelationLocation = (String, djls_source::File, Span, Option<Span>);

fn expected_span(source: &str, needle: &str) -> Result<Span, io::Error> {
    let start = source
        .find(needle)
        .ok_or_else(|| io::Error::other(format!("source does not contain {needle:?}")))?;
    Ok(Span::saturating_from_parts_usize(start, needle.len()))
}

fn expected_span_after(source: &str, anchor: &str, needle: &str) -> Result<Span, io::Error> {
    let anchor_start = source
        .find(anchor)
        .ok_or_else(|| io::Error::other(format!("source does not contain anchor {anchor:?}")))?;
    let relative_start = source[anchor_start..].find(needle).ok_or_else(|| {
        io::Error::other(format!(
            "source does not contain {needle:?} after {anchor:?}"
        ))
    })?;
    Ok(Span::saturating_from_parts_usize(
        anchor_start + relative_start,
        needle.len(),
    ))
}

fn relation_location<'a>(
    locations: &'a [RelationLocation],
    field: &str,
) -> Result<&'a RelationLocation, io::Error> {
    locations
        .iter()
        .find(|(name, ..)| name == field)
        .ok_or_else(|| io::Error::other(format!("relation location for {field:?} does not exist")))
}

fn assert_relation_location(
    actual: &RelationLocation,
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
) -> Option<u32> {
    let graph = compute_model_graph(db, project);
    let mut checksum = u32::try_from(graph.len()).ok()?;

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

    Some(checksum)
}

fn probe_model(
    db: &TestDatabase,
    project: Project,
    module_name: &str,
    model_name: &str,
) -> Option<u32> {
    model_graph_span_probe(db, project, module_name.to_string(), model_name.to_string())
}

#[test]
fn recovered_syntax_retains_imports_and_model_facts_with_error_span() {
    let source =
        "from django.db import models\n\nclass Post(models.Model):\n    pass\n\ndef broken(";
    let db = TestDatabase::new();
    let path = Utf8Path::new("/project/blog/models.py");
    db.add_file(path.as_str(), source)
        .expect("recovered model fixture should be added to the test database");
    let file = db
        .file(path)
        .expect("recovered model fixture should exist in the test database");
    let module_name =
        PythonModuleName::parse("blog.models").expect("test Python module name should be valid");

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
        Span::new(
            u32::try_from(source.len()).expect("expected JSON value should be a string"),
            0
        )
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
        .build(&db)
        .expect("shifted-model-span project fixture should build");

    let before = probe_model(&db, project, "accounts.models", "User")
        .expect("test model graph should fit in u32");
    drop(
        event_log
            .take()
            .expect("Salsa event log should be readable before the model edit"),
    );

    update_file(
        &mut db,
        "/project/accounts/models.py",
        include_str!(
            "testdata/model_relations/model_graph_span_probe_reexecutes_when_model_span_shifts/accounts/models_updated.py"
        ),
    )
    .expect("updated model fixture should be written");
    let after = probe_model(&db, project, "accounts.models", "User")
        .expect("test model graph should fit in u32");
    let events = event_log
        .take()
        .expect("Salsa event log should be readable after the model edit");

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
        .build(&db)
        .expect("added-relation project fixture should build");

    let before = probe_model(&db, project, "blog.models", "Post")
        .expect("test model graph should fit in u32");
    drop(
        event_log
            .take()
            .expect("Salsa event log should be readable before the relation edit"),
    );

    update_file(
        &mut db,
        "/project/blog/models.py",
        include_str!(
            "testdata/model_relations/model_graph_span_probe_reexecutes_when_relation_is_added/blog/models_updated.py"
        ),
    )
    .expect("updated relation fixture should be written");
    let after = probe_model(&db, project, "blog.models", "Post")
        .expect("test model graph should fit in u32");
    let events = event_log
        .take()
        .expect("Salsa event log should be readable after the relation edit");

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
        .build(&db)
        .expect("trailing-whitespace project fixture should build");

    let before = probe_model(&db, project, "accounts.models", "User")
        .expect("test model graph should fit in u32");
    drop(
        event_log
            .take()
            .expect("Salsa event log should be readable before the whitespace edit"),
    );

    update_file(
        &mut db,
        "/project/accounts/models.py",
        concat!(
            include_str!(
                "testdata/model_relations/model_graph_span_probe_backdates_for_trailing_whitespace/accounts/models_initial.py"
            ),
            "   \n"
        ),
    )
    .expect("whitespace-only model fixture should be written");
    let after = probe_model(&db, project, "accounts.models", "User")
        .expect("test model graph should fit in u32");
    let events = event_log
        .take()
        .expect("Salsa event log should be readable after the whitespace edit");

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
        .build(&db)
        .expect("model-free project fixture should build");

    let before = probe_model(&db, project, "empty.models", "Missing")
        .expect("test model graph should fit in u32");
    drop(
        event_log
            .take()
            .expect("Salsa event log should be readable before the model-free edit"),
    );

    update_file(
        &mut db,
        "/project/empty/models.py",
        include_str!(
            "testdata/model_relations/model_graph_span_probe_backdates_for_span_shift_in_file_without_model_facts/empty/models_updated.py"
        ),
    )
    .expect("updated model-free fixture should be written");
    let after = probe_model(&db, project, "empty.models", "Missing")
        .expect("test model graph should fit in u32");
    let events = event_log
        .take()
        .expect("Salsa event log should be readable after the model-free edit");

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
        .build(&db)
        .expect("deferred-child project fixture should build");

    let before = probe_model(&db, project, "blog.models", "Article")
        .expect("test model graph should fit in u32");
    drop(
        event_log
            .take()
            .expect("Salsa event log should be readable before the inherited-relation edit"),
    );

    update_file(
        &mut db,
        "/project/base/models.py",
        include_str!(
            "testdata/model_relations/model_graph_span_probe_reexecutes_when_deferred_child_inherits_shifted_relation_span/base/models_updated.py"
        ),
    )
    .expect("updated inherited-relation fixture should be written");
    let after = probe_model(&db, project, "blog.models", "Article")
        .expect("test model graph should fit in u32");
    let events = event_log
        .take()
        .expect("Salsa event log should be readable after the inherited-relation edit");

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
        .build(&db)
        .expect("relation-provenance project fixture should build");

    let graph = compute_model_graph(&db, project);
    let blog_file = db
        .file(Utf8Path::new("/project/blog/models.py"))
        .expect("blog model fixture should exist in the test database");
    let (model_file, model_span) =
        model_location(graph, "blog.models", "Post").expect("Post model should have location");
    assert_eq!(model_file, blog_file);
    assert_eq!(
        model_span,
        expected_span(source, "Post").expect("Post model name should occur in the model fixture")
    );

    let locations = model_relation_locations(graph, "blog.models", "Post");

    assert_relation_location(
        relation_location(&locations, "author").expect("author relation location should exist"),
        "author",
        blog_file,
        expected_span(source, "author").expect("author field should occur in the model fixture"),
        Some(
            expected_span(source, "\"accounts.User\"")
                .expect("author target should occur in the model fixture"),
        ),
    );
    assert_relation_location(
        relation_location(&locations, "editor").expect("editor relation location should exist"),
        "editor",
        blog_file,
        expected_span(source, "editor").expect("editor field should occur in the model fixture"),
        Some(
            expected_span(source, "account_models.User")
                .expect("editor target should occur in the model fixture"),
        ),
    );
    assert_relation_location(
        relation_location(&locations, "parent").expect("parent relation location should exist"),
        "parent",
        blog_file,
        expected_span(source, "parent").expect("parent field should occur in the model fixture"),
        Some(
            expected_span(source, "\"self\"")
                .expect("parent target should occur in the model fixture"),
        ),
    );
    assert_relation_location(
        relation_location(&locations, "tags").expect("tags relation location should exist"),
        "tags",
        blog_file,
        expected_span(source, "tags").expect("tags field should occur in the model fixture"),
        Some(
            expected_span(source, "\"Tag\"")
                .expect("tags target should occur in the model fixture"),
        ),
    );
    assert_relation_location(
        relation_location(&locations, "content_object")
            .expect("content_object relation location should exist"),
        "content_object",
        blog_file,
        expected_span(source, "content_object")
            .expect("content_object field should occur in the model fixture"),
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
        .build(&db)
        .expect("inherited-relation project fixture should build");

    let graph = compute_model_graph(&db, project);
    let same_file = db
        .file(Utf8Path::new("/project/inheritance/models.py"))
        .expect("same-file inheritance fixture should exist in the test database");
    let base_file = db
        .file(Utf8Path::new("/project/base/models.py"))
        .expect("base model fixture should exist in the test database");

    for child in ["SameFileChild", "SameFileSibling"] {
        let locations = model_relation_locations(graph, "inheritance.models", child);

        assert_relation_location(
            relation_location(&locations, "grand_owner")
                .expect("inherited grand_owner relation location should exist"),
            "grand_owner",
            same_file,
            expected_span(same_file_source, "grand_owner")
                .expect("grand_owner field should occur in the inheritance fixture"),
            Some(
                expected_span(same_file_source, "\"inheritance.GrandTarget\"")
                    .expect("grand_owner target should occur in the inheritance fixture"),
            ),
        );
        assert_relation_location(
            relation_location(&locations, "parent_owner")
                .expect("inherited parent_owner relation location should exist"),
            "parent_owner",
            same_file,
            expected_span(same_file_source, "parent_owner")
                .expect("parent_owner field should occur in the inheritance fixture"),
            Some(
                expected_span(same_file_source, "\"inheritance.ParentTarget\"")
                    .expect("parent_owner target should occur in the inheritance fixture"),
            ),
        );
    }

    for child in ["CrossChild", "CrossSibling"] {
        let locations = model_relation_locations(graph, "child.models", child);

        assert_relation_location(
            relation_location(&locations, "base_owner")
                .expect("inherited base_owner relation location should exist"),
            "base_owner",
            base_file,
            expected_span(base_source, "base_owner")
                .expect("base_owner field should occur in the base model fixture"),
            Some(
                expected_span(base_source, "\"base.BaseTarget\"")
                    .expect("base_owner target should occur in the base model fixture"),
            ),
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
        .build(&db)
        .expect("qualified-relation project fixture should build");

    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models")
        .expect("model fixture should contain the requested model");
    let accounts_user = model_id(graph, "User", "accounts.models")
        .expect("model fixture should contain the requested model");
    let blog_user = model_id(graph, "User", "blog.models")
        .expect("model fixture should contain the requested model");

    let resolved = graph
        .resolve_relation(post, "author")
        .expect("qualified relation should resolve");
    assert!(ptr::eq(
        resolved,
        graph
            .get_by_id(accounts_user)
            .expect("test value should resolve")
    ));
    assert!(!ptr::eq(
        resolved,
        graph
            .get_by_id(blog_user)
            .expect("test value should resolve")
    ));
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
        .build(&db)
        .expect("bare-relation project fixture should build");

    let graph = compute_model_graph(&db, project);
    let profile = model_id(graph, "Profile", "accounts.models")
        .expect("model fixture should contain the requested model");
    let accounts_user = model_id(graph, "User", "accounts.models")
        .expect("model fixture should contain the requested model");
    let blog_user = model_id(graph, "User", "blog.models")
        .expect("model fixture should contain the requested model");

    let resolved = graph
        .resolve_relation(profile, "user")
        .expect("bare relation should resolve in the scope app");
    assert!(ptr::eq(
        resolved,
        graph
            .get_by_id(accounts_user)
            .expect("test value should resolve")
    ));
    assert!(!ptr::eq(
        resolved,
        graph
            .get_by_id(blog_user)
            .expect("test value should resolve")
    ));
}

#[test]
fn keyword_relation_target_resolves_through_project_graph() {
    let source = r"
from django.db import models

class User(models.Model):
    pass

class Profile(models.Model):
    user = models.ForeignKey(to=User, on_delete=models.CASCADE)
";
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/accounts/models.py", source)
        .build(&db)
        .expect("keyword-relation project fixture should build");

    let graph = compute_model_graph(&db, project);
    let profile = model_id(graph, "Profile", "accounts.models")
        .expect("model fixture should contain Profile");
    let user =
        model_id(graph, "User", "accounts.models").expect("model fixture should contain User");
    let resolved = graph
        .resolve_relation(profile, "user")
        .expect("keyword relation should resolve");
    assert!(ptr::eq(
        resolved,
        graph
            .get_by_id(user)
            .expect("keyword relation target should resolve")
    ));

    let file = db
        .file(Utf8Path::new("/project/accounts/models.py"))
        .expect("keyword-relation model file should exist");
    let locations = model_relation_locations(graph, "accounts.models", "Profile");
    assert_relation_location(
        relation_location(&locations, "user").expect("user relation location should exist"),
        "user",
        file,
        expected_span_after(source, "class Profile", "user")
            .expect("user field should occur in the Profile fixture"),
        Some(
            expected_span_after(source, "class Profile", "User")
                .expect("User target should occur in the Profile fixture"),
        ),
    );
}

#[test]
fn trailing_plus_related_names_create_no_reverse_accessor() {
    let source = r#"
from django.db import models

class User(models.Model):
    pass

class Order(models.Model):
    user = models.ForeignKey(User, on_delete=models.CASCADE, related_name="orders")

class HiddenOrder(models.Model):
    user = models.ForeignKey(User, on_delete=models.CASCADE, related_name="+")

class TemplatedHiddenOrder(models.Model):
    user = models.ForeignKey(User, on_delete=models.CASCADE, related_name="%(class)s+")
"#;
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/accounts/models.py", source)
        .build(&db)
        .expect("related-name suppression fixture should build");

    let graph = compute_model_graph(&db, project);
    let user =
        model_id(graph, "User", "accounts.models").expect("model fixture should contain User");

    let order =
        model_id(graph, "Order", "accounts.models").expect("model fixture should contain Order");
    let hidden_order = model_id(graph, "HiddenOrder", "accounts.models")
        .expect("model fixture should contain HiddenOrder");
    let templated_hidden_order = model_id(graph, "TemplatedHiddenOrder", "accounts.models")
        .expect("model fixture should contain TemplatedHiddenOrder");
    let resolved = graph
        .resolve_relation(user, "orders")
        .expect("ordinary related_name should create a reverse accessor");
    assert!(ptr::eq(
        resolved,
        graph
            .get_by_id(order)
            .expect("ordinary reverse relation target should resolve")
    ));

    for absent_accessor in [
        "+",
        "hiddenorder+",
        "templatedhiddenorder+",
        "hiddenorder_set",
        "templatedhiddenorder_set",
    ] {
        assert_eq!(graph.resolve_relation(user, absent_accessor), None);
    }

    for source in [hidden_order, templated_hidden_order] {
        let resolved = graph
            .resolve_relation(source, "user")
            .expect("suppression should not affect forward resolution");
        assert!(ptr::eq(
            resolved,
            graph
                .get_by_id(user)
                .expect("suppressed relation target should resolve")
        ));
    }
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
        .build(&db)
        .expect("self-relation project fixture should build");

    let graph = compute_model_graph(&db, project);
    let category = model_id(graph, "Category", "catalog.models")
        .expect("model fixture should contain the requested model");

    let resolved = graph
        .resolve_relation(category, "parent")
        .expect("self relation should resolve");
    assert!(ptr::eq(
        resolved,
        graph
            .get_by_id(category)
            .expect("test value should resolve")
    ));
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
        .build(&db)
        .expect("imported-foreign-key project fixture should build");

    // Occurrence-local resolution follows Python scoping: the later
    // `class User` rebinds the name imported from `accounts.models`, so the FK
    // occurrence resolves to the local `blog.models.User`, not the import.
    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models")
        .expect("model fixture should contain the requested model");
    let accounts_user = model_id(graph, "User", "accounts.models")
        .expect("model fixture should contain the requested model");
    let blog_user = model_id(graph, "User", "blog.models")
        .expect("model fixture should contain the requested model");

    let resolved = graph
        .resolve_relation(post, "author")
        .expect("shadowed relation should resolve to the local class");
    assert!(ptr::eq(
        resolved,
        graph
            .get_by_id(blog_user)
            .expect("test value should resolve")
    ));
    assert!(!ptr::eq(
        resolved,
        graph
            .get_by_id(accounts_user)
            .expect("test value should resolve")
    ));

    let value = graph_value(graph).expect("model graph should serialize");
    let relation = relation_value(&value, "blog.models.Post", "author")
        .expect("model fixture should contain the requested relation");
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
        .build(&db)
        .expect("qualified-expression project fixture should build");

    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models")
        .expect("model fixture should contain the requested model");
    let accounts_user = model_id(graph, "User", "accounts.models")
        .expect("model fixture should contain the requested model");
    let blog_user = model_id(graph, "User", "blog.models")
        .expect("model fixture should contain the requested model");

    let resolved = graph
        .resolve_relation(post, "author")
        .expect("attribute relation should resolve");
    assert!(ptr::eq(
        resolved,
        graph
            .get_by_id(accounts_user)
            .expect("test value should resolve")
    ));
    assert!(!ptr::eq(
        resolved,
        graph
            .get_by_id(blog_user)
            .expect("test value should resolve")
    ));

    let value = graph_value(graph).expect("model graph should serialize");
    let relation = relation_value(&value, "blog.models.Post", "author")
        .expect("model fixture should contain the requested relation");
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
        .build(&db)
        .expect("model-base spelling project fixture should build");

    let graph = compute_model_graph(&db, project);
    model_id(graph, "FromBase", "base_alias.models")
        .expect("model fixture should contain the requested model");
    model_id(graph, "FromModule", "base_alias.models")
        .expect("model fixture should contain the requested model");
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
        .build(&db)
        .expect("abstract-base spelling project fixture should build");

    let graph = compute_model_graph(&db, project);
    let user = model_id(graph, "User", "a_app.models")
        .expect("model fixture should contain the requested model");
    let article = model_id(graph, "Article", "b_app.models")
        .expect("model fixture should contain the requested model");
    let story = model_id(graph, "Story", "c_app.models")
        .expect("model fixture should contain the requested model");

    assert!(ptr::eq(
        graph
            .resolve_relation(article, "owner")
            .expect("direct imported abstract base relation should resolve"),
        graph.get_by_id(user).expect("test value should resolve")
    ));
    assert!(ptr::eq(
        graph
            .resolve_relation(story, "owner")
            .expect("aliased imported abstract base relation should resolve"),
        graph.get_by_id(user).expect("test value should resolve")
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
        .build(&db)
        .expect("dotted-auth-user project fixture should build");

    let graph = compute_model_graph(&db, project);
    let order = model_id(graph, "Order", "shop.models")
        .expect("model fixture should contain the requested model");
    let auth_user = model_id(graph, "User", "auth.models")
        .expect("model fixture should contain the requested model");
    let shop_user = model_id(graph, "User", "shop.models")
        .expect("model fixture should contain the requested model");

    let resolved = graph
        .resolve_relation(order, "user")
        .expect("dotted string relation should resolve by app label");
    assert!(ptr::eq(
        resolved,
        graph
            .get_by_id(auth_user)
            .expect("test value should resolve")
    ));
    assert!(!ptr::eq(
        resolved,
        graph
            .get_by_id(shop_user)
            .expect("test value should resolve")
    ));

    let value = graph_value(graph).expect("model graph should serialize");
    let relation = relation_value(&value, "shop.models.Order", "user")
        .expect("model fixture should contain the requested relation");
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
        .build(&db)
        .expect("unresolved-import project fixture should build");

    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models")
        .expect("model fixture should contain the requested model");
    assert_eq!(graph.resolve_relation(post, "author"), None);

    let value = graph_value(graph).expect("model graph should serialize");
    let relation = relation_value(&value, "blog.models.Post", "author")
        .expect("model fixture should contain the requested relation");
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
        .build(&db)
        .expect("module-target project fixture should build");

    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models")
        .expect("model fixture should contain the requested model");
    assert_eq!(graph.resolve_relation(post, "author"), None);

    let value = graph_value(graph).expect("model graph should serialize");
    let relation = relation_value(&value, "blog.models.Post", "author")
        .expect("model fixture should contain the requested relation");
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
        .build(&db)
        .expect("partial-target project fixture should build");

    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models")
        .expect("model fixture should contain the requested model");
    assert_eq!(graph.resolve_relation(post, "author"), None);

    let value = graph_value(graph).expect("model graph should serialize");
    let relation = relation_value(&value, "blog.models.Post", "author")
        .expect("model fixture should contain the requested relation");
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
        .build(&db)
        .expect("ambiguous-app-label project fixture should build");

    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "NEWS.models")
        .expect("model fixture should contain the requested model");
    assert_eq!(graph.resolve_relation(post, "author"), None);

    let value = graph_value(graph).expect("model graph should serialize");
    let relation = relation_value(&value, "NEWS.models.Post", "author")
        .expect("model fixture should contain the requested relation");
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
        .build(&db)
        .expect("file-local-resolution project fixture should build");

    let graph = compute_model_graph(&db, project);
    let value = graph_value(graph).expect("model graph should serialize");
    assert_eq!(
        relation_value(&value, "blog.models.TaggedItem", "target")
            .expect("model fixture should contain the requested relation")["resolution"],
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
        relation_value(&value, "blog.models.TaggedItem", "content_object")
            .expect("model fixture should contain the requested relation")["resolution"],
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
    )
    .expect("accounts model fixture should be added to the test database");
    db.add_file(
        "/project/blog/models.py",
        include_str!("testdata/model_relations/salsa_recomputes_relation_resolution_for_import_edits_only_where_needed/blog/models_initial.py"),
    )
    .expect("blog model fixture should be added to the test database");
    db.add_file(
        "/project/other/models.py",
        include_str!("testdata/model_relations/salsa_recomputes_relation_resolution_for_import_edits_only_where_needed/other/models_initial.py"),
    )
    .expect("other model fixture should be added to the test database");

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
    let post = model_id(graph, "Post", "blog.models")
        .expect("model fixture should contain the requested model");
    let user = model_id(graph, "User", "accounts.models")
        .expect("model fixture should contain the requested model");
    assert!(ptr::eq(
        graph
            .resolve_relation(post, "author")
            .expect("initial import should resolve"),
        graph.get_by_id(user).expect("test value should resolve")
    ));
    drop(
        event_log
            .take()
            .expect("Salsa event log should be readable before the import edit"),
    );

    update_file(
        &mut db,
        "/project/blog/models.py",
        include_str!(
            "testdata/model_relations/salsa_recomputes_relation_resolution_for_import_edits_only_where_needed/blog/models_updated.py"
        ),
    )
    .expect("updated blog model fixture should be written");
    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models")
        .expect("model fixture should contain the requested model");
    let profile = model_id(graph, "Profile", "accounts.models")
        .expect("model fixture should contain the requested model");
    assert!(ptr::eq(
        graph
            .resolve_relation(post, "author")
            .expect("edited import should resolve"),
        graph.get_by_id(profile).expect("test value should resolve")
    ));
    assert!(
        execution_count(
            &db,
            &event_log
                .take()
                .expect("Salsa event log should be readable after the import edit"),
            "compute_model_graph",
        ) > 0
    );

    update_file(
        &mut db,
        "/project/other/models.py",
        include_str!(
            "testdata/model_relations/salsa_recomputes_relation_resolution_for_import_edits_only_where_needed/other/models_updated.py"
        ),
    )
    .expect("updated unrelated model fixture should be written");
    let _graph = compute_model_graph(&db, project);
    let events = event_log
        .take()
        .expect("Salsa event log should be readable after the unrelated edit");
    assert_eq!(execution_count(&db, &events, "extract_models"), 1);
}

/// Detect whether occurrence-local model extraction recognizes `class` as a
/// Django model in a single-file module. Exercises the source-order alias
/// scanner end to end through the `extract_models` query.
fn detects_model_result(source: &str, class: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let db = TestDatabase::new();
    let path = Utf8Path::new("/project/app/models.py");
    db.add_file(path.as_str(), source)?;
    let file = db.file(path)?;
    let module_name = PythonModuleName::parse("app.models")?;
    Ok(extract_model_graph(&db, file, module_name)
        .models_named(class)
        .next()
        .is_some())
}

macro_rules! detects_model {
    ($source:expr, $class:expr $(,)?) => {
        detects_model_result($source, $class).expect("model-detection fixture should build")
    };
}

#[test]
fn control_import_before_class_is_recognized() {
    assert!(detects_model!(
        "from django.db import models\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
}

#[test]
fn control_recognized_base_import_spellings() {
    // Direct alias, dotted unaliased, and named from-import alias all remain
    // recognized under occurrence-local resolution.
    assert!(detects_model!(
        "import django.db.models as dm\nclass Thing(dm.Model):\n    pass\n",
        "Thing",
    ));
    assert!(detects_model!(
        "import django.db.models\nclass Thing(django.db.models.Model):\n    pass\n",
        "Thing",
    ));
    assert!(detects_model!(
        "from django.db.models import Model as Base\nclass Thing(Base):\n    pass\n",
        "Thing",
    ));
}

#[test]
fn control_django_db_models_recognized_without_external_source() {
    // No accounts/django sources are provided; recognition is purely symbolic.
    assert!(detects_model!(
        "from django.db import models\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
}

#[test]
fn correction_import_after_class_does_not_affect_earlier_class() {
    assert!(!detects_model!(
        "class Thing(models.Model):\n    pass\nfrom django.db import models\n",
        "Thing",
    ));
}

#[test]
fn correction_reassignment_before_occurrence_removes_certainty() {
    assert!(!detects_model!(
        "from django.db import models\nmodels = None\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
}

#[test]
fn correction_delete_before_occurrence_removes_certainty() {
    assert!(!detects_model!(
        "from django.db import models\ndel models\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
}

#[test]
fn correction_function_definition_before_occurrence_removes_certainty() {
    assert!(!detects_model!(
        "from django.db import models\ndef models():\n    pass\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
}

#[test]
fn correction_class_definition_before_occurrence_removes_certainty() {
    assert!(!detects_model!(
        "from django.db import models\nclass models:\n    pass\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
}

#[test]
fn correction_branch_disagreement_is_conservatively_unresolved() {
    assert!(!detects_model!(
        "from django.db import models\nif flag:\n    models = None\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
}

#[test]
fn correction_try_disagreement_is_conservatively_unresolved() {
    assert!(!detects_model!(
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
    assert!(!detects_model!(source, "Early"));
    assert!(detects_model!(source, "Late"));
}

#[test]
fn correction_earlier_occurrence_stable_after_later_write() {
    let source = concat!(
        "from django.db import models\n",
        "class Early(models.Model):\n    pass\n",
        "models = None\n",
        "class Late(models.Model):\n    pass\n",
    );
    assert!(detects_model!(source, "Early"));
    assert!(!detects_model!(source, "Late"));
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
    assert!(detects_model!(source, "Inside"));
    assert!(!detects_model!(source, "Outside"));
}

#[test]
fn compound_binding_targets_invalidate_aliases_inside_their_bodies() {
    assert!(!detects_model!(
        "from django.db import models\nfor models in values:\n    class Thing(models.Model):\n        pass\n",
        "Thing",
    ));
    assert!(!detects_model!(
        "from django.db import models\nmatch value:\n    case models:\n        class Thing(models.Model):\n            pass\n",
        "Thing",
    ));
    assert!(!detects_model!(
        "from django.db import models\ntry:\n    pass\nexcept Exception as models:\n    class Thing(models.Model):\n        pass\n",
        "Thing",
    ));
    assert!(!detects_model!(
        "from django.db import models\nwith resource() as models:\n    class Thing(models.Model):\n        pass\n",
        "Thing",
    ));
}

#[test]
fn unusable_exact_relative_import_removes_stale_symbolic_alias() {
    assert!(!detects_model!(
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
        .build(&db)
        .expect("aliased-abstract-base project fixture should build");

    let graph = compute_model_graph(&db, project);
    model_id(graph, "Thing", "app.models")
        .expect("model fixture should contain the requested model");
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
        .build(&db)
        .expect("class-body relation project fixture should build");

    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models")
        .expect("model fixture should contain the requested model");
    let accounts_user = model_id(graph, "User", "accounts.models")
        .expect("model fixture should contain the requested model");
    assert_eq!(graph.resolve_relation(post, "before"), None);
    assert!(ptr::eq(
        graph
            .resolve_relation(post, "imported")
            .expect("class-local import should resolve at its occurrence"),
        graph
            .get_by_id(accounts_user)
            .expect("test value should resolve"),
    ));
    assert_eq!(graph.resolve_relation(post, "after"), None);

    let value = graph_value(graph).expect("model graph should serialize");
    assert_eq!(
        relation_value(&value, "blog.models.Post", "imported")
            .expect("model fixture should contain the requested relation")["resolution"],
        json!({ "Resolved": "accounts.models.User" })
    );
    assert_eq!(
        relation_value(&value, "blog.models.Post", "before")
            .expect("model fixture should contain the requested relation")["resolution"]["Unresolved"]
            ["reason"]["SameAppTargetNotFound"]["name"],
        json!("User")
    );
    assert_eq!(
        relation_value(&value, "blog.models.Post", "after")
            .expect("model fixture should contain the requested relation")["resolution"]["Unresolved"]
            ["reason"]["MissingImportBinding"],
        json!({ "binding": "User" })
    );

    let file = db
        .file(Utf8Path::new("/project/blog/models.py"))
        .expect("class-body model fixture should exist in the test database");
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
            expected_span_after(source, anchor, field)
                .expect("relation field should occur after its fixture anchor"),
            Some(
                expected_span_after(source, anchor, "User")
                    .expect("relation target should occur after its fixture anchor"),
            ),
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
        .build(&db)
        .expect("compound-alias project fixture should build");

    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models")
        .expect("model fixture should contain the requested model");
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
        assert!(detects_model!(source, name), "{name} should be recognized");
    }
}

#[test]
fn all_exact_write_forms_invalidate_symbolic_aliases() {
    assert!(!detects_model!(
        "from django.db import models\nmodels: object = None\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
    assert!(!detects_model!(
        "from django.db import models\nmodels += other\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
    assert!(!detects_model!(
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
    assert!(!detects_model!(source, "Inner"));
    assert!(detects_model!(source, "Outer"));
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
        .build(&db)
        .expect("deferred-imported-base project fixture should build");

    let graph = compute_model_graph(&db, project);
    model_id(graph, "Child", "blog.models")
        .expect("model fixture should contain the requested model");
}

#[test]
fn loop_and_match_writes_invalidate_aliases_after_the_compound() {
    assert!(!detects_model!(
        "from django.db import models\nfor item in values:\n    models = None\nclass Thing(models.Model):\n    pass\n",
        "Thing",
    ));
    assert!(!detects_model!(
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
        .build(&db)
        .expect("string-relation project fixture should build");

    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models")
        .expect("model fixture should contain the requested model");
    let blog_user = model_id(graph, "User", "blog.models")
        .expect("model fixture should contain the requested model");
    assert!(ptr::eq(
        graph
            .resolve_relation(post, "author")
            .expect("string target should resolve in the current app"),
        graph
            .get_by_id(blog_user)
            .expect("test value should resolve"),
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
        .build(&db)
        .expect("shadowed-relation project fixture should build");

    let graph = compute_model_graph(&db, project);
    let post = model_id(graph, "Post", "blog.models")
        .expect("model fixture should contain the requested model");
    assert_eq!(graph.resolve_relation(post, "author"), None);
    let value = graph_value(graph).expect("model graph should serialize");
    assert_eq!(
        relation_value(&value, "blog.models.Post", "author")
            .expect("model fixture should contain the requested relation")["resolution"]["Unresolved"]
            ["reason"]["MissingImportBinding"],
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
    assert!(!detects_model!(source, "Child"));
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
        .build(&db)
        .expect("deferred-model project fixture should build");

    let graph = compute_model_graph(&db, project);
    let thing = model_id(graph, "Thing", "blog.models")
        .expect("model fixture should contain the requested model");
    assert_eq!(graph.resolve_relation(thing, "old"), None);
    assert!(graph.resolve_relation(thing, "new").is_some());
    let (_file, span) =
        model_location(graph, "blog.models", "Thing").expect("test value should resolve");
    assert_eq!(
        span,
        expected_span_after(source, "class Thing(models.Model)", "Thing")
            .expect("later Thing model name should occur after its class declaration")
    );
}

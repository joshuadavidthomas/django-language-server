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
use djls_project::testing::ModelAncestryOutcomeView;
use djls_project::testing::ModelBaseOutcomeView;
use djls_project::testing::ModelBaseUnresolvedReasonView;
use djls_project::testing::ModelInvalidAncestryReasonView;
use djls_project::testing::ModelMroEntryView;
use djls_project::testing::PythonSyntaxErrorClass;
use djls_project::testing::extract_model_graph;
use djls_project::testing::model_inheritance_outcomes;
use djls_project::testing::model_location;
use djls_project::testing::model_relation_locations;
use djls_project::testing::python_syntax_errors;
use djls_project::testing::resolve_model_graph_from_modules;
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
fn model_graph_span_probe_reexecutes_for_base_only_edit() {
    let initial = concat!(
        "from django.db import models\n",
        "class A(models.Model):\n    pass\n",
        "class B(models.Model):\n    pass\n",
        "class Child(A):\n    pass\n",
    );
    let updated = initial.replace("class Child(A)", "class Child(B)");
    let event_log = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(event_log.clone());
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", initial)
        .build(&db)
        .expect("base-only fixture should build");

    let before = probe_model(&db, project, "app.models", "Child")
        .expect("test model graph should fit in u32");
    drop(
        event_log
            .take()
            .expect("Salsa event log should be readable before the base edit"),
    );
    update_file(&mut db, "/project/app/models.py", &updated)
        .expect("updated base fixture should be written");
    let after = probe_model(&db, project, "app.models", "Child")
        .expect("test model graph should fit in u32");
    let events = event_log
        .take()
        .expect("Salsa event log should be readable after the base edit");

    assert_eq!(after, before);
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
fn class_body_conditional_relation_remains_a_possible_field() {
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
    let user = model_id(graph, "User", "accounts.models")
        .expect("model fixture should contain the conditional relation target");
    assert!(ptr::eq(
        graph
            .resolve_relation(post, "conditional")
            .expect("conditional relation should remain a possible field"),
        graph
            .get_by_id(user)
            .expect("conditional relation target should resolve"),
    ));
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
fn unresolved_only_model_is_retained_with_ordered_typed_base_outcomes() {
    let source = concat!(
        "from django.db import models\n",
        "class Target(models.Model):\n    pass\n",
        "class Child(missing.AbstractBase, make_base()):\n",
        "    direct = models.ForeignKey(Target, on_delete=models.CASCADE)\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("unresolved-only inheritance fixture should build");

    let graph = compute_model_graph(&db, project);
    let child = model_id(graph, "Child", "app.models")
        .expect("an unresolved-only candidate should remain in the graph");
    assert!(graph.resolve_relation(child, "direct").is_some());

    let (bases, ancestry) = model_inheritance_outcomes(graph, "app.models", "Child")
        .expect("Child should retain inheritance outcomes");
    assert_eq!(bases.len(), 2);
    assert!(matches!(
        &bases[0],
        ModelBaseOutcomeView::Unresolved {
            span,
            reason: ModelBaseUnresolvedReasonView::MissingImportBinding { path },
        } if *span == expected_span(source, "missing.AbstractBase").expect("base should occur")
            && path == &["missing".to_string(), "AbstractBase".to_string()]
    ));
    assert!(matches!(
        &bases[1],
        ModelBaseOutcomeView::Unresolved {
            span,
            reason: ModelBaseUnresolvedReasonView::UnsupportedExpression,
        } if *span == expected_span(source, "make_base()").expect("unsupported base should occur")
    ));
    assert_eq!(ancestry, ModelAncestryOutcomeView::Partial);
}

#[test]
fn relation_evidence_retains_partial_model_without_promoting_arbitrary_subclass() {
    let source = concat!(
        "from django.db import models\n",
        "class Target(models.Model):\n    pass\n",
        "class BaseFormatter:\n    pass\n",
        "class Formatter(BaseFormatter):\n    pass\n",
        "class WebhookBase:\n    pass\n",
        "class Webhook(WebhookBase):\n",
        "    target = models.ForeignKey(Target, on_delete=models.CASCADE)\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("candidate-admission fixture should build");

    let graph = compute_model_graph(&db, project);
    assert!(model_id(graph, "Formatter", "app.models").is_err());
    let webhook =
        model_id(graph, "Webhook", "app.models").expect("relation evidence should retain Webhook");
    assert!(graph.resolve_relation(webhook, "target").is_some());

    let (bases, ancestry) = model_inheritance_outcomes(graph, "app.models", "Webhook")
        .expect("Webhook should retain its plain-class base outcome");
    assert!(matches!(
        bases.as_slice(),
        [ModelBaseOutcomeView::NonModelClass { class, .. }]
            if class.name() == "WebhookBase"
    ));
    assert!(matches!(
        ancestry,
        ModelAncestryOutcomeView::Complete { ref mro }
            if matches!(
                mro.as_slice(),
                [
                    ModelMroEntryView::Model(model),
                    ModelMroEntryView::NonModelClass(class),
                ] if model.name() == "Webhook" && class.name() == "WebhookBase"
            )
    ));
}

#[test]
fn plain_same_file_mixin_keeps_local_ancestry_complete() {
    let source = concat!(
        "from django.db import models\n",
        "class PlainMixin:\n    pass\n",
        "class Child(PlainMixin, models.Model):\n    pass\n",
    );
    let db = TestDatabase::new();
    let path = Utf8Path::new("/project/app/models.py");
    db.add_file(path.as_str(), source)
        .expect("plain-mixin fixture should be added to the test database");
    let file = db
        .file(path)
        .expect("plain-mixin fixture should exist in the test database");
    let module_name =
        PythonModuleName::parse("app.models").expect("test module name should be valid");

    let graph = extract_model_graph(&db, file, module_name);
    assert!(model_id(&graph, "PlainMixin", "app.models").is_err());
    let (bases, ancestry) = model_inheritance_outcomes(&graph, "app.models", "Child")
        .expect("Child should retain its plain-class base outcome");
    assert!(matches!(
        &bases[0],
        ModelBaseOutcomeView::NonModelClass { class, .. } if class.name() == "PlainMixin"
    ));
    assert!(matches!(
        ancestry,
        ModelAncestryOutcomeView::Complete { ref mro }
            if matches!(
                mro.as_slice(),
                [
                    ModelMroEntryView::Model(model),
                    ModelMroEntryView::NonModelClass(class),
                ] if model.name() == "Child" && class.name() == "PlainMixin"
            )
    ));
}

#[test]
fn plain_mixin_local_name_blocks_an_older_abstract_relation() {
    let source = concat!(
        "from django.db import models\n",
        "class Target(models.Model):\n    pass\n",
        "class AbstractBase(models.Model):\n",
        "    owner = models.ForeignKey(Target)\n",
        "    class Meta:\n        abstract = True\n",
        "class PlainMixin:\n    owner = None\n",
        "class Child(PlainMixin, AbstractBase):\n    pass\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("plain-mixin shadow fixture should build");

    let graph = compute_model_graph(&db, project);
    let child = model_id(graph, "Child", "app.models").expect("Child should exist");
    assert_eq!(graph.resolve_relation(child, "owner"), None);
    let (_bases, ancestry) = model_inheritance_outcomes(graph, "app.models", "Child")
        .expect("Child should retain complete ancestry");
    assert!(matches!(
        ancestry,
        ModelAncestryOutcomeView::Complete { .. }
    ));
}

#[test]
fn unresolved_plain_mixin_ancestry_propagates_partial() {
    let source = concat!(
        "from django.db import models\n",
        "class BrokenMixin(missing.Base):\n    pass\n",
        "class Child(BrokenMixin, models.Model):\n    pass\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("unresolved plain-mixin fixture should build");

    let graph = compute_model_graph(&db, project);
    let (bases, ancestry) = model_inheritance_outcomes(graph, "app.models", "Child")
        .expect("Child should retain its plain-class base outcome");
    assert!(matches!(
        &bases[0],
        ModelBaseOutcomeView::NonModelClass { class, .. } if class.name() == "BrokenMixin"
    ));
    assert_eq!(ancestry, ModelAncestryOutcomeView::Partial);
}

#[test]
fn same_file_abstract_order_resolves_through_plain_mixins() {
    let source = concat!(
        "from django.db import models\n",
        "class StatusMixin:\n    pass\n",
        "class TransitionMixin:\n    pass\n",
        "class Order(StatusMixin, TransitionMixin, models.Model):\n",
        "    class Meta:\n        abstract = True\n",
        "class PurchaseOrder(Order):\n    pass\n",
    );
    let db = TestDatabase::new();
    let path = Utf8Path::new("/project/order/models.py");
    db.add_file(path.as_str(), source)
        .expect("abstract-order fixture should be added to the test database");
    let file = db
        .file(path)
        .expect("abstract-order fixture should exist in the test database");
    let module_name =
        PythonModuleName::parse("order.models").expect("test module name should be valid");

    let graph = extract_model_graph(&db, file, module_name);
    for model_name in ["Order", "PurchaseOrder"] {
        let (_bases, ancestry) = model_inheritance_outcomes(&graph, "order.models", model_name)
            .expect("same-file model should retain inheritance outcomes");
        assert!(
            matches!(ancestry, ModelAncestryOutcomeView::Complete { .. }),
            "{model_name} ancestry should be complete"
        );
    }
}

#[test]
fn mixed_resolved_and_unresolved_bases_keep_only_direct_fields() {
    let source = concat!(
        "from django.db import models\n",
        "class Target(models.Model):\n    pass\n",
        "class AbstractBase(models.Model):\n",
        "    inherited = models.ForeignKey(Target, on_delete=models.CASCADE)\n",
        "    class Meta:\n        abstract = True\n",
        "class Child(AbstractBase, missing.OtherBase):\n",
        "    direct = models.ForeignKey(Target, on_delete=models.CASCADE)\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("mixed inheritance fixture should build");

    let graph = compute_model_graph(&db, project);
    let child = model_id(graph, "Child", "app.models").expect("Child should remain present");
    assert!(graph.resolve_relation(child, "direct").is_some());
    assert_eq!(graph.resolve_relation(child, "inherited"), None);

    let (bases, ancestry) = model_inheritance_outcomes(graph, "app.models", "Child")
        .expect("Child should retain inheritance outcomes");
    assert!(
        matches!(&bases[0], ModelBaseOutcomeView::Model { model, .. } if model.name() == "AbstractBase")
    );
    assert!(matches!(
        &bases[1],
        ModelBaseOutcomeView::Unresolved {
            reason: ModelBaseUnresolvedReasonView::MissingImportBinding { .. },
            ..
        }
    ));
    assert_eq!(ancestry, ModelAncestryOutcomeView::Partial);
}

#[test]
fn partial_qualified_base_target_is_retained() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/base/models.py",
            "from django.db import models\nclass Root(models.Model):\n    pass\n",
        )
        .file(
            "/project/app/models.py",
            concat!(
                "import base.models as base_models\n",
                "class Child(base_models.nested.AbstractBase):\n",
                "    class Meta:\n",
                "        abstract = True\n",
            ),
        )
        .build(&db)
        .expect("partial qualified base fixture should build");

    let graph = compute_model_graph(&db, project);
    model_id(graph, "Child", "app.models").expect("partial candidate should remain present");
    let (bases, ancestry) = model_inheritance_outcomes(graph, "app.models", "Child")
        .expect("Child should retain inheritance outcomes");
    assert!(matches!(
        bases.as_slice(),
        [ModelBaseOutcomeView::Unresolved {
            reason: ModelBaseUnresolvedReasonView::PartialImport {
                resolved_prefix,
                unresolved_tail,
            },
            ..
        }] if resolved_prefix.as_str() == "base.models"
            && unresolved_tail == &["nested".to_string(), "AbstractBase".to_string()]
    ));
    assert_eq!(ancestry, ModelAncestryOutcomeView::Partial);
}

#[test]
fn resolved_empty_mixin_does_not_hide_an_unresolved_field_base() {
    let source = concat!(
        "from django.db import models\n",
        "class Target(models.Model):\n    pass\n",
        "class EmptyMixin(models.Model):\n",
        "    class Meta:\n        abstract = True\n",
        "class Child(EmptyMixin, missing.FieldBase):\n",
        "    direct = models.ForeignKey(Target, on_delete=models.CASCADE)\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("empty-mixin inheritance fixture should build");

    let graph = compute_model_graph(&db, project);
    let child = model_id(graph, "Child", "app.models").expect("Child should remain present");
    assert!(graph.resolve_relation(child, "direct").is_some());
    let (bases, ancestry) = model_inheritance_outcomes(graph, "app.models", "Child")
        .expect("Child should retain both base outcomes");
    assert_eq!(bases.len(), 2);
    assert!(
        matches!(&bases[0], ModelBaseOutcomeView::Model { model, .. } if model.name() == "EmptyMixin")
    );
    assert!(matches!(&bases[1], ModelBaseOutcomeView::Unresolved { .. }));
    assert_eq!(ancestry, ModelAncestryOutcomeView::Partial);
}

#[test]
fn c3_left_base_wins_a_forward_field_collision() {
    let source = concat!(
        "from django.db import models\n",
        "class LeftTarget(models.Model):\n    pass\n",
        "class RightTarget(models.Model):\n    pass\n",
        "class Left(models.Model):\n",
        "    owner = models.ForeignKey(LeftTarget, on_delete=models.CASCADE)\n",
        "    class Meta:\n        abstract = True\n",
        "class Right(models.Model):\n",
        "    owner = models.ForeignKey(RightTarget, on_delete=models.CASCADE)\n",
        "    class Meta:\n        abstract = True\n",
        "class Child(Left, Right):\n    pass\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("C3 collision fixture should build");

    let graph = compute_model_graph(&db, project);
    let child = model_id(graph, "Child", "app.models").expect("Child should exist");
    let left_target = model_id(graph, "LeftTarget", "app.models").expect("LeftTarget should exist");
    let resolved = graph
        .resolve_relation(child, "owner")
        .expect("the left C3 field should resolve");
    assert!(ptr::eq(
        resolved,
        graph
            .get_by_id(left_target)
            .expect("LeftTarget should resolve")
    ));
}

#[test]
fn known_c3_conflict_propagates_through_partial_ancestry() {
    let source = concat!(
        "class X:\n    pass\n",
        "class Y:\n    pass\n",
        "class A(X, Y, missing.Base):\n",
        "    class Meta:\n        abstract = True\n",
        "class B(Y, X):\n",
        "    class Meta:\n        abstract = True\n",
        "class C(A, B):\n",
        "    class Meta:\n        abstract = True\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("partial-parent C3 fixture should build");
    let graph = compute_model_graph(&db, project);

    let (a_bases, a_ancestry) = model_inheritance_outcomes(graph, "app.models", "A")
        .expect("A should remain admitted with partial ancestry");
    assert!(matches!(
        &a_bases[2],
        ModelBaseOutcomeView::Unresolved {
            reason: ModelBaseUnresolvedReasonView::MissingImportBinding { .. },
            ..
        }
    ));
    assert_eq!(a_ancestry, ModelAncestryOutcomeView::Partial);

    let (_c_bases, c_ancestry) = model_inheritance_outcomes(graph, "app.models", "C")
        .expect("C should remain admitted with invalid ancestry");
    assert_eq!(
        c_ancestry,
        ModelAncestryOutcomeView::Invalid {
            reason: ModelInvalidAncestryReasonView::InconsistentMethodResolutionOrder,
        }
    );
}

#[test]
fn inherited_concrete_field_target_uses_the_declaring_models_scope() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/base/models.py",
            concat!(
                "from django.db import models\n",
                "class Target(models.Model):\n    pass\n",
                "class Parent(models.Model):\n",
                "    target = models.ForeignKey(\"Target\", on_delete=models.CASCADE)\n",
            ),
        )
        .file(
            "/project/child/models.py",
            concat!(
                "from django.db import models\n",
                "from base.models import Parent\n",
                "class Target(models.Model):\n    pass\n",
                "class Child(Parent):\n    pass\n",
            ),
        )
        .build(&db)
        .expect("owner-scope inheritance fixture should build");

    let graph = compute_model_graph(&db, project);
    let child = model_id(graph, "Child", "child.models").expect("Child should exist");
    let base_target = model_id(graph, "Target", "base.models").expect("base Target should exist");
    let child_target =
        model_id(graph, "Target", "child.models").expect("child Target should exist");
    let resolved = graph
        .resolve_relation(child, "target")
        .expect("inherited concrete field should resolve");
    assert!(ptr::eq(
        resolved,
        graph
            .get_by_id(base_target)
            .expect("base Target should resolve")
    ));
    assert!(!ptr::eq(
        resolved,
        graph
            .get_by_id(child_target)
            .expect("child Target should resolve")
    ));
    assert!(model_relation_locations(graph, "child.models", "Child").is_empty());
}

#[test]
fn a_base_declared_later_still_produces_complete_ancestry() {
    let source = concat!(
        "from django.db import models\n",
        "class Target(models.Model):\n    pass\n",
        "class Child(LateBase):\n    pass\n",
        "class LateBase(models.Model):\n",
        "    target = models.ForeignKey(Target, on_delete=models.CASCADE)\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("late-base fixture should build");

    let graph = compute_model_graph(&db, project);
    let child = model_id(graph, "Child", "app.models").expect("Child should exist");
    assert!(graph.resolve_relation(child, "target").is_some());
    let (_bases, ancestry) = model_inheritance_outcomes(graph, "app.models", "Child")
        .expect("Child should retain complete ancestry");
    assert!(matches!(
        ancestry,
        ModelAncestryOutcomeView::Complete { .. }
    ));
}

#[test]
fn a_same_module_base_rebound_after_the_child_is_unresolved() {
    let source = concat!(
        "from django.db import models\n",
        "class Target(models.Model):\n    pass\n",
        "class Base(models.Model):\n",
        "    old = models.ForeignKey(Target)\n",
        "    class Meta:\n        abstract = True\n",
        "class Child(Base):\n    pass\n",
        "class Base(models.Model):\n",
        "    new = models.ForeignKey(Target)\n",
        "    class Meta:\n        abstract = True\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("rebound local base fixture should build");
    let graph = compute_model_graph(&db, project);
    let child = model_id(graph, "Child", "app.models").expect("Child should remain present");

    assert_eq!(graph.resolve_relation(child, "old"), None);
    assert_eq!(graph.resolve_relation(child, "new"), None);
    let (bases, ancestry) = model_inheritance_outcomes(graph, "app.models", "Child")
        .expect("Child should retain inheritance outcomes");
    assert!(matches!(
        bases.as_slice(),
        [ModelBaseOutcomeView::Unresolved {
            reason: ModelBaseUnresolvedReasonView::ReboundLocalBase { class },
            ..
        }] if class.name() == "Base" && class.module_name().as_str() == "app.models"
    ));
    assert_eq!(ancestry, ModelAncestryOutcomeView::Partial);
}

#[test]
fn rebound_local_base_admission_uses_proven_same_module_ancestry() {
    let source = concat!(
        "from django.db import models\n",
        "class Root(models.Model):\n",
        "    class Meta:\n        abstract = True\n",
        "class Base(Root):\n    pass\n",
        "class Child(Base):\n    pass\n",
        "class Base:\n    pass\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("ancestral rebound local base fixture should build");
    let graph = compute_model_graph(&db, project);

    model_id(graph, "Child", "app.models").expect("proven same-module ancestry should admit Child");
    let (bases, ancestry) = model_inheritance_outcomes(graph, "app.models", "Child")
        .expect("Child should retain inheritance outcomes");
    assert!(matches!(
        bases.as_slice(),
        [ModelBaseOutcomeView::Unresolved {
            reason: ModelBaseUnresolvedReasonView::ReboundLocalBase { class },
            ..
        }] if class.name() == "Base" && class.module_name().as_str() == "app.models"
    ));
    assert_eq!(ancestry, ModelAncestryOutcomeView::Partial);
}

#[test]
fn a_plain_rebound_local_base_does_not_admit_its_child() {
    let source = concat!(
        "from django.db import models\n",
        "class Base:\n    pass\n",
        "class Child(Base):\n    pass\n",
        "class Base(models.Model):\n    pass\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("plain rebound local base fixture should build");
    let graph = compute_model_graph(&db, project);

    assert!(model_id(graph, "Child", "app.models").is_err());
    assert!(model_id(graph, "Base", "app.models").is_ok());
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

#[test]
fn sole_unsupported_base_candidate_is_retained_with_relation_evidence() {
    let source = concat!(
        "from django.db import models\n",
        "class Child(make_base()):\n",
        "    parent = models.ForeignKey(\"self\", on_delete=models.CASCADE)\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("unsupported-base fixture should build");

    let graph = compute_model_graph(&db, project);
    model_id(graph, "Child", "app.models").expect("unsupported candidate should remain present");
    let (bases, ancestry) = model_inheritance_outcomes(graph, "app.models", "Child")
        .expect("unsupported candidate should retain its base outcome");
    assert!(matches!(
        bases.as_slice(),
        [ModelBaseOutcomeView::Unresolved {
            reason: ModelBaseUnresolvedReasonView::UnsupportedExpression,
            ..
        }]
    ));
    assert_eq!(ancestry, ModelAncestryOutcomeView::Partial);
}

#[test]
fn later_search_root_deferred_duplicate_wins() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/low/duplicate/models.py",
            concat!(
                "from django.db import models\n",
                "class Base(models.Model):\n    class Meta:\n        abstract = True\n",
                "class Thing(Base):\n    low = models.ForeignKey(\"self\")\n",
            ),
        )
        .file(
            "/high/duplicate/models.py",
            concat!(
                "from django.db import models\n",
                "class Base(models.Model):\n    class Meta:\n        abstract = True\n",
                "class Thing(Base):\n    high = models.ForeignKey(\"self\")\n",
            ),
        )
        .build(&db)
        .expect("duplicate-root fixture should build");
    let low = db
        .file(Utf8Path::new("/low/duplicate/models.py"))
        .expect("low-priority module should exist");
    let high = db
        .file(Utf8Path::new("/high/duplicate/models.py"))
        .expect("high-priority module should exist");
    let module = PythonModuleName::parse("duplicate.models").expect("module name should parse");

    // This is the order produced by reversing search-path discovery: the
    // higher-priority root arrives last and must replace the earlier candidate.
    let graph =
        resolve_model_graph_from_modules(&db, project, [(low, module.clone()), (high, module)]);
    let thing = model_id(&graph, "Thing", "duplicate.models").expect("Thing should exist");
    assert_eq!(graph.resolve_relation(thing, "low"), None);
    assert!(graph.resolve_relation(thing, "high").is_some());
    assert_eq!(
        model_location(&graph, "duplicate.models", "Thing")
            .expect("winner should have a location")
            .0,
        high
    );
}

#[test]
fn django_root_before_derived_base_is_invalid() {
    let source = concat!(
        "from django.db import models\n",
        "class Target(models.Model):\n    pass\n",
        "class AbstractBase(models.Model):\n",
        "    inherited = models.ForeignKey(Target)\n",
        "    class Meta:\n        abstract = True\n",
        "class Bad(models.Model, AbstractBase):\n    pass\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("root-order fixture should build");
    let graph = compute_model_graph(&db, project);
    let bad = model_id(graph, "Bad", "app.models").expect("Bad should remain present");
    let (_bases, ancestry) = model_inheritance_outcomes(graph, "app.models", "Bad")
        .expect("Bad should retain invalid ancestry");
    assert_eq!(
        ancestry,
        ModelAncestryOutcomeView::Invalid {
            reason: ModelInvalidAncestryReasonView::InconsistentMethodResolutionOrder,
        }
    );
    assert_eq!(graph.resolve_relation(bad, "inherited"), None);
}

#[test]
fn duplicate_class_base_is_invalid() {
    let source = concat!(
        "from django.db import models\n",
        "class Base(models.Model):\n    pass\n",
        "class Child(Base, Base):\n    pass\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("duplicate-base fixture should build");
    let graph = compute_model_graph(&db, project);
    let (_bases, ancestry) = model_inheritance_outcomes(graph, "app.models", "Child")
        .expect("Child should retain invalid ancestry");
    assert!(matches!(
        ancestry,
        ModelAncestryOutcomeView::Invalid {
            reason: ModelInvalidAncestryReasonView::DuplicateClassBase { ref class },
        } if class.name() == "Base"
    ));
}

#[test]
fn cycle_wins_over_unresolved_ancestry() {
    let source = concat!(
        "class A(B, missing.Base):\n",
        "    class Meta:\n",
        "        abstract = True\n",
        "class B(A):\n    pass\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("cycle fixture should build");
    let graph = compute_model_graph(&db, project);
    let (bases, ancestry) = model_inheritance_outcomes(graph, "app.models", "A")
        .expect("A should retain invalid ancestry");
    assert!(matches!(
        &bases[1],
        ModelBaseOutcomeView::Unresolved {
            reason: ModelBaseUnresolvedReasonView::MissingImportBinding { .. },
            ..
        }
    ));
    assert_eq!(
        ancestry,
        ModelAncestryOutcomeView::Invalid {
            reason: ModelInvalidAncestryReasonView::Cycle,
        }
    );
}

#[test]
fn abstract_clone_stops_at_concrete_owner_scope() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/base/models.py",
            concat!(
                "from django.db import models\n",
                "class Target(models.Model):\n    pass\n",
                "class AbstractBase(models.Model):\n",
                "    target = models.ForeignKey(\"Target\")\n",
                "    class Meta:\n        abstract = True\n",
                "class ConcreteParent(AbstractBase):\n    pass\n",
            ),
        )
        .file(
            "/project/child/models.py",
            concat!(
                "from django.db import models\n",
                "from base.models import ConcreteParent\n",
                "class Target(models.Model):\n    pass\n",
                "class Child(ConcreteParent):\n    pass\n",
            ),
        )
        .build(&db)
        .expect("abstract concrete chain should build");
    let graph = compute_model_graph(&db, project);
    let child = model_id(graph, "Child", "child.models").expect("Child should exist");
    let base_target = model_id(graph, "Target", "base.models").expect("base Target should exist");
    let resolved = graph
        .resolve_relation(child, "target")
        .expect("inherited field should resolve");
    assert!(ptr::eq(
        resolved,
        graph
            .get_by_id(base_target)
            .expect("base target should resolve")
    ));
    assert!(model_relation_locations(graph, "child.models", "Child").is_empty());
}

#[test]
fn abstract_clone_expression_target_keeps_its_declaration_scope() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/project/base/models.py",
            concat!(
                "from django.db import models\n",
                "class Target(models.Model):\n    pass\n",
                "class AbstractBase(models.Model):\n",
                "    target = models.ForeignKey(Target)\n",
                "    class Meta:\n        abstract = True\n",
            ),
        )
        .file(
            "/project/child/models.py",
            concat!(
                "from django.db import models\n",
                "from base.models import AbstractBase\n",
                "class Target(models.Model):\n    pass\n",
                "class Child(AbstractBase):\n    pass\n",
            ),
        )
        .build(&db)
        .expect("cross-app abstract clone fixture should build");
    let graph = compute_model_graph(&db, project);
    let child = model_id(graph, "Child", "child.models").expect("Child should exist");
    let base_target = model_id(graph, "Target", "base.models").expect("base Target should exist");
    let child_target =
        model_id(graph, "Target", "child.models").expect("child Target should exist");

    assert!(ptr::eq(
        graph
            .resolve_relation(child, "target")
            .expect("cloned expression target should resolve"),
        graph
            .get_by_id(base_target)
            .expect("base Target should resolve"),
    ));
    assert!(ptr::eq(
        graph
            .resolve_relation(base_target, "child_set")
            .expect("base Target should receive the cloned reverse relation"),
        graph.get_by_id(child).expect("Child should resolve"),
    ));
    assert_eq!(graph.resolve_relation(child_target, "child_set"), None);
}

#[test]
fn extracted_class_bindings_block_abstract_relation_cloning() {
    let source = concat!(
        "from django.db import models\n",
        "class Target(models.Model):\n    pass\n",
        "class AbstractBase(models.Model):\n",
        "    owner = models.ForeignKey(Target)\n",
        "    class Meta:\n        abstract = True\n",
        "class AssignedChild(AbstractBase):\n    owner = None\n",
        "class MethodChild(AbstractBase):\n    def owner(self):\n        pass\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("local-shadow fixture should build");
    let graph = compute_model_graph(&db, project);
    for name in ["AssignedChild", "MethodChild"] {
        let child = model_id(graph, name, "app.models").expect("child should exist");
        assert_eq!(graph.resolve_relation(child, "owner"), None);
        assert!(model_relation_locations(graph, "app.models", name).is_empty());
    }
}

#[test]
fn abstract_declarations_do_not_create_reverse_descriptors() {
    let source = concat!(
        "from django.db import models\n",
        "class User(models.Model):\n    pass\n",
        "class AbstractBase(models.Model):\n",
        "    user = models.ForeignKey(User)\n",
        "    editor = models.ForeignKey(User, related_name=\"%(class)s_editors\")\n",
        "    class Meta:\n        abstract = True\n",
        "class Child(AbstractBase):\n    pass\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("abstract reverse fixture should build");
    let graph = compute_model_graph(&db, project);
    let user = model_id(graph, "User", "app.models").expect("User should exist");
    assert!(graph.resolve_relation(user, "child_set").is_some());
    assert!(graph.resolve_relation(user, "child_editors").is_some());
    assert_eq!(graph.resolve_relation(user, "abstractbase_set"), None);
    assert_eq!(graph.resolve_relation(user, "abstractbase_editors"), None);
}

#[test]
fn c3_uses_local_bindings_across_a_shared_ancestor_diamond() {
    let source = concat!(
        "from django.db import models\n",
        "class XTarget(models.Model):\n    pass\n",
        "class CTarget(models.Model):\n    pass\n",
        "class X(models.Model):\n",
        "    owner = models.ForeignKey(XTarget)\n",
        "class B(X):\n    pass\n",
        "class C(X):\n",
        "    owner = models.ForeignKey(CTarget)\n",
        "class D(B, C):\n    pass\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("shared-ancestor C3 fixture should build");
    let graph = compute_model_graph(&db, project);
    let d = model_id(graph, "D", "app.models").expect("D should exist");
    let c_target = model_id(graph, "CTarget", "app.models").expect("CTarget should exist");

    assert!(ptr::eq(
        graph
            .resolve_relation(d, "owner")
            .expect("D.owner should resolve"),
        graph.get_by_id(c_target).expect("CTarget should resolve"),
    ));
}

#[test]
fn later_non_relation_suppresses_an_earlier_relation() {
    let source = concat!(
        "from django.db import models\n",
        "class User(models.Model):\n    pass\n",
        "class Order(models.Model):\n",
        "    user = models.ForeignKey(User, related_name=\"orders\")\n",
        "    user = None\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("relation suppression fixture should build");
    let graph = compute_model_graph(&db, project);
    let order = model_id(graph, "Order", "app.models").expect("Order should exist");
    let value = graph_value(graph).expect("model graph should serialize");

    assert_eq!(graph.resolve_relation(order, "user"), None);
    assert!(
        value["models"]["app.models.Order"]["relations"]
            .as_array()
            .is_some_and(Vec::is_empty)
    );
}

#[test]
fn later_relation_restores_a_name_after_a_non_relation() {
    let source = concat!(
        "from django.db import models\n",
        "class User(models.Model):\n    pass\n",
        "class Order(models.Model):\n",
        "    user = None\n",
        "    user = models.ForeignKey(User)\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("relation restoration fixture should build");
    let graph = compute_model_graph(&db, project);
    let order = model_id(graph, "Order", "app.models").expect("Order should exist");

    assert!(graph.resolve_relation(order, "user").is_some());
}

#[test]
fn later_relation_replaces_an_earlier_relation() {
    let source = concat!(
        "from django.db import models\n",
        "class First(models.Model):\n    pass\n",
        "class Second(models.Model):\n    pass\n",
        "class Choice(models.Model):\n",
        "    selected = models.ForeignKey(First)\n",
        "    selected = models.ForeignKey(Second)\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("relation replacement fixture should build");
    let graph = compute_model_graph(&db, project);
    let choice = model_id(graph, "Choice", "app.models").expect("Choice should exist");
    let second = model_id(graph, "Second", "app.models").expect("Second should exist");
    let value = graph_value(graph).expect("model graph should serialize");
    let relations = value["models"]["app.models.Choice"]["relations"]
        .as_array()
        .expect("Choice relations should be an array");

    assert!(ptr::eq(
        graph
            .resolve_relation(choice, "selected")
            .expect("the final relation should resolve"),
        graph.get_by_id(second).expect("Second should resolve"),
    ));
    assert_eq!(relations.len(), 1);
    assert_eq!(relations[0]["target"]["value"]["name"], json!("Second"));
}

#[test]
fn suppressed_local_relation_creates_no_reverse_descriptor() {
    let source = concat!(
        "from django.db import models\n",
        "class User(models.Model):\n    pass\n",
        "class Order(models.Model):\n",
        "    user = models.ForeignKey(User, related_name=\"orders\")\n",
        "    user = None\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("reverse suppression fixture should build");
    let graph = compute_model_graph(&db, project);
    let user = model_id(graph, "User", "app.models").expect("User should exist");

    assert_eq!(graph.resolve_relation(user, "orders"), None);
}

#[test]
fn final_static_meta_abstract_assignment_wins() {
    let source = concat!(
        "from django.db import models\n",
        "class Concrete(models.Model):\n",
        "    class Meta:\n",
        "        abstract = True\n",
        "        abstract = False\n",
        "class Abstract(models.Model):\n",
        "    class Meta:\n",
        "        abstract = False\n",
        "        abstract = True\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("Meta.abstract ordering fixture should build");
    let value = graph_value(compute_model_graph(&db, project)).expect("graph should serialize");

    assert_eq!(value["models"]["app.models.Concrete"]["kind"], "concrete");
    assert_eq!(value["models"]["app.models.Abstract"]["kind"], "abstract");
}

#[test]
fn final_duplicate_meta_class_controls_model_kind() {
    let source = concat!(
        "from django.db import models\n",
        "class ResetToDefault(models.Model):\n",
        "    class Meta:\n        abstract = True\n",
        "    class Meta:\n        pass\n",
        "class ResetToFalse(models.Model):\n",
        "    class Meta:\n        abstract = True\n",
        "    class Meta:\n        abstract = False\n",
        "class ResetToTrue(models.Model):\n",
        "    class Meta:\n        abstract = False\n",
        "    class Meta:\n        abstract = True\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("duplicate Meta fixture should build");
    let value = graph_value(compute_model_graph(&db, project)).expect("graph should serialize");

    assert_eq!(
        value["models"]["app.models.ResetToDefault"]["kind"],
        "concrete"
    );
    assert_eq!(
        value["models"]["app.models.ResetToFalse"]["kind"],
        "concrete"
    );
    assert_eq!(
        value["models"]["app.models.ResetToTrue"]["kind"],
        "abstract"
    );
}

#[test]
fn reassigned_model_alias_cannot_be_admitted_by_a_relation() {
    let source = concat!(
        "from django.db import models\n",
        "class Target(models.Model):\n    pass\n",
        "models = None\n",
        "class Shadowed(models.Model):\n",
        "    target = models.ForeignKey(Target)\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("reassigned alias fixture should build");
    let graph = compute_model_graph(&db, project);

    assert!(model_id(graph, "Shadowed", "app.models").is_err());
}

#[test]
fn shadowed_direct_model_import_is_negative_django_root_evidence() {
    let source = concat!(
        "from django.db.models import Model, ForeignKey\n",
        "Model = object()\n",
        "class Fake(Model):\n",
        "    parent = ForeignKey(\"self\")\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("shadowed direct Model import fixture should build");
    let graph = compute_model_graph(&db, project);

    assert!(model_id(graph, "Fake", "app.models").is_err());
}

#[test]
fn missing_direct_model_name_is_negative_django_root_evidence() {
    let source = concat!(
        "class Fake(Model):\n",
        "    parent = ForeignKey(\"self\")\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("missing direct Model fixture should build");
    let graph = compute_model_graph(&db, project);

    assert!(model_id(graph, "Fake", "app.models").is_err());
}

#[test]
fn proven_model_base_can_admit_a_candidate_with_negative_model_evidence() {
    let source = concat!(
        "from django.db import models\n",
        "class Target(models.Model):\n    pass\n",
        "class ProvenBase(models.Model):\n    pass\n",
        "models = None\n",
        "class Child(ProvenBase, models.Model):\n",
        "    target = ForeignKey(Target)\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("proven alternate base fixture should build");
    let graph = compute_model_graph(&db, project);
    let child =
        model_id(graph, "Child", "app.models").expect("the proven model base should admit Child");

    assert!(graph.resolve_relation(child, "target").is_some());
}

#[test]
fn deleted_and_branch_shadowed_model_aliases_are_negative_evidence() {
    let source = concat!(
        "from django.db import models\n",
        "class Target(models.Model):\n    pass\n",
        "del models\n",
        "class Deleted(models.Model):\n",
        "    target = object()\n",
        "    relation = ForeignKey(Target)\n",
        "from django.db import models\n",
        "if enabled:\n    models = None\n",
        "class BranchShadowed(models.Model):\n",
        "    target = ForeignKey(Target)\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("uncertain alias fixture should build");
    let graph = compute_model_graph(&db, project);

    assert!(model_id(graph, "Deleted", "app.models").is_err());
    assert!(model_id(graph, "BranchShadowed", "app.models").is_err());
}

#[test]
fn later_reimport_admits_only_later_relation_bearing_models() {
    let source = concat!(
        "from django.db import models\n",
        "class Target(models.Model):\n    pass\n",
        "models = None\n",
        "class Before(models.Model):\n",
        "    target = ForeignKey(Target)\n",
        "from django.db import models\n",
        "class After(models.Model):\n",
        "    target = models.ForeignKey(Target)\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("alias reimport fixture should build");
    let graph = compute_model_graph(&db, project);

    assert!(model_id(graph, "Before", "app.models").is_err());
    assert!(model_id(graph, "After", "app.models").is_ok());
}

#[test]
fn later_alias_shadow_does_not_change_an_earlier_candidate() {
    let source = concat!(
        "from django.db import models\n",
        "class Target(models.Model):\n    pass\n",
        "class Before(models.Model):\n",
        "    target = models.ForeignKey(Target)\n",
        "models = None\n",
        "class After(models.Model):\n",
        "    target = ForeignKey(Target)\n",
    );
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file("/project/app/models.py", source)
        .build(&db)
        .expect("later alias shadow fixture should build");
    let graph = compute_model_graph(&db, project);

    assert!(model_id(graph, "Before", "app.models").is_ok());
    assert!(model_id(graph, "After", "app.models").is_err());
}

#[test]
fn final_module_file_hides_all_lower_priority_classes() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/low/app/models.py",
            concat!(
                "from django.db import models\n",
                "class Legacy(models.Model):\n    pass\n",
            ),
        )
        .file(
            "/high/app/models.py",
            concat!(
                "from django.db import models\n",
                "class Current(models.Model):\n    pass\n",
            ),
        )
        .build(&db)
        .expect("module precedence fixture should build");
    let low = db
        .file(Utf8Path::new("/low/app/models.py"))
        .expect("low-priority module should exist");
    let high = db
        .file(Utf8Path::new("/high/app/models.py"))
        .expect("high-priority module should exist");
    let module = PythonModuleName::parse("app.models").expect("module should parse");
    let graph =
        resolve_model_graph_from_modules(&db, project, [(low, module.clone()), (high, module)]);

    assert!(model_id(&graph, "Legacy", "app.models").is_err());
    assert!(model_id(&graph, "Current", "app.models").is_ok());
}

#[test]
fn model_free_winning_module_hides_a_lower_model() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .file(
            "/low/app/models.py",
            "from django.db import models\nclass Legacy(models.Model):\n    pass\n",
        )
        .file("/high/app/models.py", "class Helper:\n    pass\n")
        .build(&db)
        .expect("model-free module precedence fixture should build");
    let low = db
        .file(Utf8Path::new("/low/app/models.py"))
        .expect("low-priority module should exist");
    let high = db
        .file(Utf8Path::new("/high/app/models.py"))
        .expect("high-priority module should exist");
    let module = PythonModuleName::parse("app.models").expect("module should parse");
    let graph =
        resolve_model_graph_from_modules(&db, project, [(low, module.clone()), (high, module)]);

    assert!(graph.is_empty());
}

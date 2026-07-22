use std::io;

use camino::Utf8Path;
use djls_conf::DiagnosticSeverity;
use djls_conf::DiagnosticsConfig;
use djls_ide::code_actions;
use djls_ide::collect_diagnostics;
use djls_source::LineCol;
use djls_source::LineIndex;
use djls_source::PositionEncoding;
use djls_source::Span;
use djls_testing::TestDatabase;
use djls_testing::standard_validation_db;
use tower_lsp_server::ls_types;

const TEMPLATE_PATH: &str = "/test/project/templates/template.html";

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

fn db_with_source(source: &str) -> TestResult<TestDatabase> {
    db_with_source_and_config(source, DiagnosticsConfig::default())
}

fn db_with_source_and_config(
    source: &str,
    diagnostics_config: DiagnosticsConfig,
) -> TestResult<TestDatabase> {
    let db = standard_validation_db()?.with_diagnostics_config(diagnostics_config);
    db.add_file(TEMPLATE_PATH, source)?;
    Ok(db)
}

fn file(db: &TestDatabase) -> Result<djls_source::File, djls_source::FileError> {
    db.file(Utf8Path::new(TEMPLATE_PATH))
}

fn template_uri() -> TestResult<ls_types::Uri> {
    Ok(
        ls_types::Uri::from_file_path(Utf8Path::new(TEMPLATE_PATH).as_std_path())
            .ok_or_else(|| io::Error::other("template path should convert to a file URI"))?,
    )
}

fn request_at(source: &str, needle: &str) -> TestResult<Span> {
    let offset = source
        .find(needle)
        .ok_or_else(|| io::Error::other(format!("test source should contain {needle:?}")))?;
    Ok(Span::new(u32::try_from(offset)?, 0))
}

fn collect_actions(
    db: &TestDatabase,
    range: TestResult<Span>,
) -> TestResult<Vec<ls_types::CodeAction>> {
    code_actions(db, file(db)?, range?, PositionEncoding::Utf16)
        .ok_or_else(|| io::Error::other("template file should return a code action response"))?
        .into_iter()
        .map(|action| match action {
            ls_types::CodeActionOrCommand::CodeAction(action) => Ok(action),
            ls_types::CodeActionOrCommand::Command(_) => {
                Err(io::Error::other("expected code action").into())
            }
        })
        .collect()
}

fn only_action(actions: Vec<ls_types::CodeAction>) -> TestResult<ls_types::CodeAction> {
    let action_count = actions.len();
    let [action] = actions.try_into().map_err(|_actions: Vec<_>| {
        io::Error::other(format!("expected one action, got {action_count}"))
    })?;
    Ok(action)
}

fn only_edit(action: &ls_types::CodeAction) -> TestResult<&ls_types::TextEdit> {
    let edit = action
        .edit
        .as_ref()
        .ok_or_else(|| io::Error::other("code action should have an edit"))?;
    let changes = edit
        .changes
        .as_ref()
        .ok_or_else(|| io::Error::other("code action should use WorkspaceEdit.changes"))?;
    let edits = changes
        .get(&template_uri()?)
        .ok_or_else(|| io::Error::other("workspace edit should target the template URI"))?;
    let edit_count = edits.len();
    let [edit]: &[ls_types::TextEdit; 1] = edits.as_slice().try_into().map_err(|error| {
        io::Error::other(format!("expected one text edit, got {edit_count}: {error}"))
    })?;
    Ok(edit)
}

fn apply_edit(source: &str, edit: &ls_types::TextEdit) -> String {
    let line_index = LineIndex::from(source);
    let start = line_index
        .offset(
            source,
            LineCol::new(edit.range.start.line, edit.range.start.character),
            PositionEncoding::Utf16,
        )
        .get() as usize;
    let end = line_index
        .offset(
            source,
            LineCol::new(edit.range.end.line, edit.range.end.character),
            PositionEncoding::Utf16,
        )
        .get() as usize;

    let mut updated = String::with_capacity(source.len() + edit.new_text.len());
    updated.push_str(&source[..start]);
    updated.push_str(&edit.new_text);
    updated.push_str(&source[end..]);
    updated
}

fn diagnostic_codes(source: &str) -> TestResult<Vec<String>> {
    let db = db_with_source(source)?;
    Ok(collect_diagnostics(&db, file(&db)?)
        .ok_or_else(|| io::Error::other("template file should return diagnostics"))?
        .into_iter()
        .filter_map(|diagnostic| match diagnostic.code {
            Some(ls_types::NumberOrString::String(code)) => Some(code),
            Some(ls_types::NumberOrString::Number(code)) => Some(code.to_string()),
            None => None,
        })
        .collect())
}

#[test]
fn unloaded_tag_action_inserts_load_after_import_header() {
    let source = "{% extends \"base.html\" %}\n{% load static %}\n{% trans \"Hi\" %}\n";
    let db = db_with_source(source).expect("validation fixture should build");
    let actions = collect_actions(&db, request_at(source, "trans"))
        .expect("unloaded tag should produce a code action response");
    let action = only_action(actions).expect("unloaded tag should produce one action");
    let edit = only_edit(&action).expect("unloaded tag action should contain one edit");

    assert_eq!(action.title, "Add '{% load i18n %}'");
    assert_eq!(action.kind, Some(ls_types::CodeActionKind::QUICKFIX));
    assert_eq!(action.is_preferred, Some(true));
    assert_eq!(edit.range.start, ls_types::Position::new(2, 0));
    assert_eq!(edit.range.end, ls_types::Position::new(2, 0));
    assert_eq!(edit.new_text, "{% load i18n %}\n");
    assert_eq!(
        diagnostic_codes(&apply_edit(source, edit))
            .expect("edited template diagnostics should be collected"),
        Vec::<String>::new()
    );
}

#[test]
fn unloaded_tag_action_inserts_load_at_top_without_header() {
    let source = "{% trans \"Hi\" %}\n";
    let db = db_with_source(source).expect("validation fixture should build");
    let actions = collect_actions(&db, request_at(source, "trans"))
        .expect("unloaded tag should produce a code action response");
    let action = only_action(actions).expect("unloaded tag should produce one action");
    let edit = only_edit(&action).expect("unloaded tag action should contain one edit");

    assert_eq!(edit.range.start, ls_types::Position::new(0, 0));
    assert_eq!(edit.range.end, ls_types::Position::new(0, 0));
    assert_eq!(edit.new_text, "{% load i18n %}\n");
    assert_eq!(
        diagnostic_codes(&apply_edit(source, edit))
            .expect("edited template diagnostics should be collected"),
        Vec::<String>::new()
    );
}

#[test]
fn unloaded_filter_action_inserts_required_library() {
    let source = "{{ value|trans }}\n";
    let db = db_with_source(source).expect("validation fixture should build");
    let actions = collect_actions(&db, request_at(source, "trans"))
        .expect("unloaded filter should produce a code action response");
    let action = only_action(actions).expect("unloaded filter should produce one action");
    let edit = only_edit(&action).expect("unloaded filter action should contain one edit");

    assert_eq!(action.title, "Add '{% load i18n %}'");
    assert_eq!(edit.range.start, ls_types::Position::new(0, 0));
    assert_eq!(edit.range.end, ls_types::Position::new(0, 0));
    assert_eq!(edit.new_text, "{% load i18n %}\n");
    assert_eq!(
        diagnostic_codes(&apply_edit(source, edit))
            .expect("edited template diagnostics should be collected"),
        Vec::<String>::new()
    );
}

#[test]
fn insert_load_action_preserves_crlf_line_endings() {
    let source = "{% extends \"base.html\" %}\r\n{% trans \"Hi\" %}\r\n";
    let db = db_with_source(source).expect("validation fixture should build");
    let actions = collect_actions(&db, request_at(source, "trans"))
        .expect("unloaded tag should produce a code action response");
    let action = only_action(actions).expect("unloaded tag should produce one action");
    let edit = only_edit(&action).expect("unloaded tag action should contain one edit");

    assert_eq!(edit.range.start, ls_types::Position::new(1, 0));
    assert_eq!(edit.range.end, ls_types::Position::new(1, 0));
    assert_eq!(edit.new_text, "{% load i18n %}\r\n");
    assert_eq!(
        diagnostic_codes(&apply_edit(source, edit))
            .expect("edited template diagnostics should be collected"),
        Vec::<String>::new()
    );
}

#[test]
fn ambiguous_unloaded_tag_actions_offer_each_library_in_order() {
    let source = "{% shared %}\n";
    let db = db_with_source(source).expect("validation fixture should build");
    let actions = collect_actions(&db, request_at(source, "shared"))
        .expect("ambiguous unloaded tag should produce a code action response");

    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0].title, "Add '{% load alpha %}'");
    assert_eq!(actions[1].title, "Add '{% load beta %}'");
    assert_eq!(actions[0].is_preferred, None);
    assert_eq!(actions[1].is_preferred, None);

    for action in actions {
        let edit = only_edit(&action).expect("unloaded tag action should contain one edit");
        assert_eq!(edit.range.start, ls_types::Position::new(0, 0));
        assert_eq!(edit.range.end, ls_types::Position::new(0, 0));
        assert_eq!(
            diagnostic_codes(&apply_edit(source, edit))
                .expect("edited template diagnostics should be collected"),
            Vec::<String>::new()
        );
    }
}

#[test]
fn ambiguous_unloaded_filter_actions_offer_each_library_in_order() {
    let source = "{{ value|shared_filter }}\n";
    let db = db_with_source(source).expect("validation fixture should build");
    let actions = collect_actions(&db, request_at(source, "shared_filter"))
        .expect("ambiguous unloaded filter should produce a code action response");

    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0].title, "Add '{% load alpha %}'");
    assert_eq!(actions[1].title, "Add '{% load beta %}'");
    assert_eq!(actions[0].is_preferred, None);
    assert_eq!(actions[1].is_preferred, None);

    for action in actions {
        let edit = only_edit(&action).expect("unloaded filter action should contain one edit");
        assert_eq!(edit.range.start, ls_types::Position::new(0, 0));
        assert_eq!(edit.range.end, ls_types::Position::new(0, 0));
        assert_eq!(
            diagnostic_codes(&apply_edit(source, edit))
                .expect("edited template diagnostics should be collected"),
            Vec::<String>::new()
        );
    }
}

#[test]
fn unmatched_block_name_action_renames_closing_block() {
    let source = "{% block content %}\n{% endblock wrong %}\n";
    let db = db_with_source(source).expect("validation fixture should build");
    let actions = collect_actions(&db, request_at(source, "wrong"))
        .expect("unmatched block should produce a code action response");
    let action = only_action(actions).expect("unmatched block should produce one action");
    let edit = only_edit(&action).expect("block rename action should contain one edit");

    assert_eq!(action.title, "Rename closing block to 'content'");
    assert_eq!(action.kind, Some(ls_types::CodeActionKind::QUICKFIX));
    assert_eq!(action.is_preferred, Some(true));
    assert_eq!(edit.range.start, ls_types::Position::new(1, 12));
    assert_eq!(edit.range.end, ls_types::Position::new(1, 17));
    assert_eq!(edit.new_text, "content");
    assert_eq!(
        diagnostic_codes(&apply_edit(source, edit))
            .expect("edited template diagnostics should be collected"),
        Vec::<String>::new()
    );
}

#[test]
fn unnamed_closing_block_returns_no_rename_action() {
    let source = "{% block content %}\n{% endblock %}\n";
    let db = db_with_source(source).expect("validation fixture should build");

    let actions = collect_actions(&db, request_at(source, "endblock"))
        .expect("unnamed closing block should return a code action response");

    assert!(actions.is_empty());
}

#[test]
fn non_intersecting_range_returns_no_actions() {
    let source = "{% trans \"Hi\" %}\nbody\n";
    let db = db_with_source(source).expect("validation fixture should build");

    let actions = collect_actions(&db, request_at(source, "body"))
        .expect("non-intersecting range should return a code action response");

    assert!(actions.is_empty());
}

#[test]
fn severity_off_suppresses_diagnostic_and_code_action() {
    let source = "{% trans \"Hi\" %}\n";
    let mut config = DiagnosticsConfig::default();
    config.set_severity("S109", DiagnosticSeverity::Off);
    let db = db_with_source_and_config(source, config)
        .expect("validation fixture with custom diagnostics should build");

    let file = file(&db).expect("template fixture file should exist");
    let diagnostics = collect_diagnostics(&db, file).expect("template should return diagnostics");
    let actions = collect_actions(&db, request_at(source, "trans"))
        .expect("disabled diagnostic should return a code action response");

    assert!(diagnostics.is_empty());
    assert!(actions.is_empty());
}

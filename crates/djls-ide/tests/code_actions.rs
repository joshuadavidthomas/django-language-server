use std::collections::HashMap;

use camino::Utf8Path;
use djls_conf::DiagnosticSeverity;
use djls_conf::DiagnosticsConfig;
use djls_ide::code_actions;
use djls_ide::collect_diagnostics;
use djls_project::TemplateLibraries;
use djls_source::LineCol;
use djls_source::LineIndex;
use djls_source::PositionEncoding;
use djls_source::Span;
use djls_testing::TestDatabase;
use djls_testing::builtin_tag;
use djls_testing::library_filter;
use djls_testing::library_tag;
use djls_testing::make_template_libraries_with_open_remainder;
use tower_lsp_server::ls_types;

const TEMPLATE_PATH: &str = "/test/project/templates/template.html";

fn template_libraries(db: &TestDatabase) -> TemplateLibraries {
    let tags = vec![
        builtin_tag("block", "django.template.loader_tags"),
        builtin_tag("endblock", "django.template.loader_tags"),
        builtin_tag("extends", "django.template.loader_tags"),
        builtin_tag("load", "django.template.defaulttags"),
        library_tag("trans", "i18n", "django.templatetags.i18n"),
        library_tag("shared", "beta", "project.templatetags.beta"),
        library_tag("shared", "alpha", "project.templatetags.alpha"),
    ];
    let filters = vec![
        library_filter("trans", "i18n", "django.templatetags.i18n"),
        library_filter("shared_filter", "beta", "project.templatetags.beta"),
        library_filter("shared_filter", "alpha", "project.templatetags.alpha"),
    ];
    let libraries = HashMap::from([
        (
            "alpha".to_string(),
            "project.templatetags.alpha".to_string(),
        ),
        ("beta".to_string(), "project.templatetags.beta".to_string()),
        ("i18n".to_string(), "django.templatetags.i18n".to_string()),
        (
            "static".to_string(),
            "django.templatetags.static".to_string(),
        ),
    ]);

    make_template_libraries_with_open_remainder(
        db,
        &tags,
        &filters,
        &libraries,
        &[
            "django.template.defaulttags".to_string(),
            "django.template.loader_tags".to_string(),
        ],
        false,
    )
}

fn db_with_source(source: &str) -> TestDatabase {
    db_with_source_and_config(source, DiagnosticsConfig::default())
}

fn db_with_source_and_config(source: &str, diagnostics_config: DiagnosticsConfig) -> TestDatabase {
    let db = TestDatabase::new();
    let libraries = template_libraries(&db);
    let db = db
        .with_template_libraries(libraries)
        .with_diagnostics_config(diagnostics_config);
    db.add_file(TEMPLATE_PATH, source);
    db
}

fn file(db: &TestDatabase) -> djls_source::File {
    db.file(Utf8Path::new(TEMPLATE_PATH))
}

fn template_uri() -> ls_types::Uri {
    ls_types::Uri::from_file_path(Utf8Path::new(TEMPLATE_PATH).as_std_path())
        .expect("template path should convert to a file URI")
}

fn request_at(source: &str, needle: &str) -> Span {
    let offset = source.find(needle).expect("needle should exist in source");
    Span::new(u32::try_from(offset).unwrap(), 0)
}

fn collect_actions(db: &TestDatabase, range: Span) -> Vec<ls_types::CodeAction> {
    code_actions(db, file(db), range, PositionEncoding::Utf16)
        .expect("template file should return a code action response")
        .into_iter()
        .map(|action| match action {
            ls_types::CodeActionOrCommand::CodeAction(action) => action,
            ls_types::CodeActionOrCommand::Command(_) => panic!("expected code action"),
        })
        .collect()
}

fn only_action(actions: Vec<ls_types::CodeAction>) -> ls_types::CodeAction {
    let [action]: [ls_types::CodeAction; 1] = actions.try_into().expect("expected one action");
    action
}

fn only_edit(action: &ls_types::CodeAction) -> &ls_types::TextEdit {
    let edit = action
        .edit
        .as_ref()
        .expect("code action should have an edit");
    let changes = edit
        .changes
        .as_ref()
        .expect("code action should use WorkspaceEdit.changes");
    let edits = changes
        .get(&template_uri())
        .expect("workspace edit should target the template URI");
    let [edit]: &[ls_types::TextEdit; 1] = edits.as_slice().try_into().unwrap();
    edit
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

fn diagnostic_codes(source: &str) -> Vec<String> {
    let db = db_with_source(source);
    collect_diagnostics(&db, file(&db))
        .expect("template file should return diagnostics")
        .into_iter()
        .filter_map(|diagnostic| match diagnostic.code {
            Some(ls_types::NumberOrString::String(code)) => Some(code),
            Some(ls_types::NumberOrString::Number(code)) => Some(code.to_string()),
            None => None,
        })
        .collect()
}

#[test]
fn unloaded_tag_action_inserts_load_after_import_header() {
    let source = "{% extends \"base.html\" %}\n{% load static %}\n{% trans \"Hi\" %}\n";
    let db = db_with_source(source);
    let action = only_action(collect_actions(&db, request_at(source, "trans")));
    let edit = only_edit(&action);

    assert_eq!(action.title, "Add '{% load i18n %}'");
    assert_eq!(action.kind, Some(ls_types::CodeActionKind::QUICKFIX));
    assert_eq!(action.is_preferred, Some(true));
    assert_eq!(edit.range.start, ls_types::Position::new(2, 0));
    assert_eq!(edit.range.end, ls_types::Position::new(2, 0));
    assert_eq!(edit.new_text, "{% load i18n %}\n");
    assert_eq!(
        diagnostic_codes(&apply_edit(source, edit)),
        Vec::<String>::new()
    );
}

#[test]
fn unloaded_tag_action_inserts_load_at_top_without_header() {
    let source = "{% trans \"Hi\" %}\n";
    let db = db_with_source(source);
    let action = only_action(collect_actions(&db, request_at(source, "trans")));
    let edit = only_edit(&action);

    assert_eq!(edit.range.start, ls_types::Position::new(0, 0));
    assert_eq!(edit.range.end, ls_types::Position::new(0, 0));
    assert_eq!(edit.new_text, "{% load i18n %}\n");
    assert_eq!(
        diagnostic_codes(&apply_edit(source, edit)),
        Vec::<String>::new()
    );
}

#[test]
fn unloaded_filter_action_inserts_required_library() {
    let source = "{{ value|trans }}\n";
    let db = db_with_source(source);
    let action = only_action(collect_actions(&db, request_at(source, "trans")));
    let edit = only_edit(&action);

    assert_eq!(action.title, "Add '{% load i18n %}'");
    assert_eq!(edit.range.start, ls_types::Position::new(0, 0));
    assert_eq!(edit.range.end, ls_types::Position::new(0, 0));
    assert_eq!(edit.new_text, "{% load i18n %}\n");
    assert_eq!(
        diagnostic_codes(&apply_edit(source, edit)),
        Vec::<String>::new()
    );
}

#[test]
fn insert_load_action_preserves_crlf_line_endings() {
    let source = "{% extends \"base.html\" %}\r\n{% trans \"Hi\" %}\r\n";
    let db = db_with_source(source);
    let action = only_action(collect_actions(&db, request_at(source, "trans")));
    let edit = only_edit(&action);

    assert_eq!(edit.range.start, ls_types::Position::new(1, 0));
    assert_eq!(edit.range.end, ls_types::Position::new(1, 0));
    assert_eq!(edit.new_text, "{% load i18n %}\r\n");
    assert_eq!(
        diagnostic_codes(&apply_edit(source, edit)),
        Vec::<String>::new()
    );
}

#[test]
fn ambiguous_unloaded_tag_actions_offer_each_library_in_order() {
    let source = "{% shared %}\n";
    let db = db_with_source(source);
    let actions = collect_actions(&db, request_at(source, "shared"));

    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0].title, "Add '{% load alpha %}'");
    assert_eq!(actions[1].title, "Add '{% load beta %}'");
    assert_eq!(actions[0].is_preferred, None);
    assert_eq!(actions[1].is_preferred, None);

    for action in actions {
        let edit = only_edit(&action);
        assert_eq!(edit.range.start, ls_types::Position::new(0, 0));
        assert_eq!(edit.range.end, ls_types::Position::new(0, 0));
        assert_eq!(
            diagnostic_codes(&apply_edit(source, edit)),
            Vec::<String>::new()
        );
    }
}

#[test]
fn ambiguous_unloaded_filter_actions_offer_each_library_in_order() {
    let source = "{{ value|shared_filter }}\n";
    let db = db_with_source(source);
    let actions = collect_actions(&db, request_at(source, "shared_filter"));

    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0].title, "Add '{% load alpha %}'");
    assert_eq!(actions[1].title, "Add '{% load beta %}'");
    assert_eq!(actions[0].is_preferred, None);
    assert_eq!(actions[1].is_preferred, None);

    for action in actions {
        let edit = only_edit(&action);
        assert_eq!(edit.range.start, ls_types::Position::new(0, 0));
        assert_eq!(edit.range.end, ls_types::Position::new(0, 0));
        assert_eq!(
            diagnostic_codes(&apply_edit(source, edit)),
            Vec::<String>::new()
        );
    }
}

#[test]
fn unmatched_block_name_action_renames_closing_block() {
    let source = "{% block content %}\n{% endblock wrong %}\n";
    let db = db_with_source(source);
    let action = only_action(collect_actions(&db, request_at(source, "wrong")));
    let edit = only_edit(&action);

    assert_eq!(action.title, "Rename closing block to 'content'");
    assert_eq!(action.kind, Some(ls_types::CodeActionKind::QUICKFIX));
    assert_eq!(action.is_preferred, Some(true));
    assert_eq!(edit.range.start, ls_types::Position::new(1, 12));
    assert_eq!(edit.range.end, ls_types::Position::new(1, 17));
    assert_eq!(edit.new_text, "content");
    assert_eq!(
        diagnostic_codes(&apply_edit(source, edit)),
        Vec::<String>::new()
    );
}

#[test]
fn unnamed_closing_block_returns_no_rename_action() {
    let source = "{% block content %}\n{% endblock %}\n";
    let db = db_with_source(source);

    let actions = collect_actions(&db, request_at(source, "endblock"));

    assert!(actions.is_empty());
}

#[test]
fn non_intersecting_range_returns_no_actions() {
    let source = "{% trans \"Hi\" %}\nbody\n";
    let db = db_with_source(source);

    let actions = collect_actions(&db, request_at(source, "body"));

    assert!(actions.is_empty());
}

#[test]
fn severity_off_suppresses_diagnostic_and_code_action() {
    let source = "{% trans \"Hi\" %}\n";
    let mut config = DiagnosticsConfig::default();
    config.set_severity("S109", DiagnosticSeverity::Off);
    let db = db_with_source_and_config(source, config);

    let diagnostics =
        collect_diagnostics(&db, file(&db)).expect("template should have diagnostics");
    let actions = collect_actions(&db, request_at(source, "trans"));

    assert!(diagnostics.is_empty());
    assert!(actions.is_empty());
}

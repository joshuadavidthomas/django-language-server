use std::borrow::Cow;
use std::collections::HashMap;

use camino::Utf8Path;
use djls_ide::completion;
use djls_project::TemplateInventoryStatus;
use djls_project::TemplateLibraries;
use djls_semantic::TagSpec;
use djls_semantic::TagSpecs;
use djls_source::Offset;
use djls_source::PositionEncoding;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;
use djls_testing::builtin_tag;
use djls_testing::library_filter;
use djls_testing::library_tag;
use djls_testing::make_template_libraries_with_status;
use tower_lsp_server::ls_types;

fn tag_libraries(db: &TestDatabase) -> TemplateLibraries {
    tag_libraries_with_status(db, TemplateInventoryStatus::Complete)
}

fn tag_libraries_with_status(
    db: &TestDatabase,
    status: TemplateInventoryStatus,
) -> TemplateLibraries {
    let tags = vec![
        builtin_tag("if", "django.template.defaulttags"),
        library_tag("trans", "i18n", "django.templatetags.i18n"),
        library_tag("blocktrans", "i18n", "django.templatetags.i18n"),
    ];
    let libraries = HashMap::from([("i18n".to_string(), "django.templatetags.i18n".to_string())]);
    let builtins = vec!["django.template.defaulttags".to_string()];

    make_template_libraries_with_status(db, &tags, &[], &libraries, &builtins, status)
}

fn filter_libraries(db: &TestDatabase) -> TemplateLibraries {
    let filters = vec![library_filter("trans", "i18n", "django.templatetags.i18n")];
    let libraries = HashMap::from([("i18n".to_string(), "django.templatetags.i18n".to_string())]);

    make_template_libraries_with_status(
        db,
        &[],
        &filters,
        &libraries,
        &[],
        TemplateInventoryStatus::Complete,
    )
}

fn project_only_specs() -> TagSpecs {
    let mut specs = TagSpecs::default();
    specs.insert(
        "project_only".to_string(),
        TagSpec::new(
            Cow::Borrowed("project.templatetags.project_only"),
            None,
            Cow::Borrowed(&[]),
            false,
        ),
    );
    specs
}

fn source_and_offset(marked_source: &str) -> (String, Offset) {
    let offset = marked_source
        .find('§')
        .expect("test source should contain a cursor marker");
    let mut source = marked_source.to_string();
    source.remove(offset);
    (source, Offset::new(u32::try_from(offset).unwrap()))
}

fn completion_items(
    marked_source: &str,
    template_libraries: impl FnOnce(&TestDatabase) -> TemplateLibraries,
    tag_specs: TagSpecs,
) -> Vec<ls_types::CompletionItem> {
    let (source, offset) = source_and_offset(marked_source);
    let db = TestDatabase::new().with_specs(tag_specs);
    let template_libraries = template_libraries(&db);
    let db = db.with_template_libraries(template_libraries);
    db.add_file("template.html", &source);
    let file = db.file(Utf8Path::new("template.html"));

    let Some(response) = completion(&db, file, offset, PositionEncoding::Utf16, false) else {
        return Vec::new();
    };

    match response {
        ls_types::CompletionResponse::Array(items) => items,
        ls_types::CompletionResponse::List(list) => list.items,
    }
}

fn completion_labels(
    marked_source: &str,
    template_libraries: impl FnOnce(&TestDatabase) -> TemplateLibraries,
    tag_specs: TagSpecs,
) -> Vec<String> {
    completion_items(marked_source, template_libraries, tag_specs)
        .into_iter()
        .map(|item| item.label)
        .collect()
}

fn install_template_completion_project(db: &mut TestDatabase, child_path: &str, source: &str) {
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates', '/test/project/app/templates'], 'APP_DIRS': False}]\n",
        )
        .template_file("child.html", child_path, source)
        .template_file("base.html", "/test/project/templates/base.html", "base")
        .template_file("shared.html", "/test/project/templates/shared.html", "primary")
        .template_file("account/detail.html", "/test/project/app/templates/account/detail.html", "detail")
        .template_file("shared.html", "/test/project/app/templates/shared.html", "shadow")
        .install(db);
}

#[test]
fn tag_completions_respect_load_position() {
    let before_load = completion_labels(
        "{% § %}\n{% load i18n %}",
        tag_libraries,
        TagSpecs::default(),
    );
    let mut after_load = completion_labels(
        "{% load i18n %}\n{% § %}",
        tag_libraries,
        TagSpecs::default(),
    );
    after_load.sort_unstable();

    assert_eq!(before_load, vec!["if"]);
    assert_eq!(after_load, vec!["blocktrans", "if", "trans"]);
}

#[test]
fn partial_tag_completions_use_known_libraries_not_raw_specs() {
    let labels = completion_labels(
        "{% project§ %}",
        |db| tag_libraries_with_status(db, TemplateInventoryStatus::Incomplete),
        project_only_specs(),
    );

    assert!(labels.is_empty());
}

#[test]
fn filter_completions_respect_load_position() {
    let before_load = completion_labels(
        "{{ value|tr§ }}\n{% load i18n %}",
        filter_libraries,
        TagSpecs::default(),
    );
    let after_load = completion_labels(
        "{% load i18n %}\n{{ value|tr§ }}",
        filter_libraries,
        TagSpecs::default(),
    );

    assert!(before_load.is_empty());
    assert_eq!(after_load, vec!["trans"]);
}

#[test]
fn template_name_completions_use_resolvable_project_names() {
    let mut db = TestDatabase::new();
    let (source, offset) = source_and_offset(r#"{% extends "§" %}"#);
    let child_path = "/test/project/templates/child.html";
    install_template_completion_project(&mut db, child_path, &source);
    let file = db.file(Utf8Path::new(child_path));

    let response = completion(&db, file, offset, PositionEncoding::Utf16, false)
        .expect("template names should complete inside quoted references");
    let items = match response {
        ls_types::CompletionResponse::Array(items) => items,
        ls_types::CompletionResponse::List(list) => list.items,
    };
    let labels = items
        .iter()
        .map(|item| item.label.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        labels,
        vec![
            "account/detail.html",
            "base.html",
            "child.html",
            "shared.html",
        ]
    );
    assert_eq!(items[0].kind, Some(ls_types::CompletionItemKind::FILE));
    assert_eq!(items[0].detail.as_deref(), Some("Django template"));
}

#[test]
fn template_name_completion_replaces_quoted_argument_interior() {
    let mut db = TestDatabase::new();
    let (source, offset) = source_and_offset(r#"{% extends "acc§ount/detail.html" %}"#);
    let child_path = "/test/project/templates/child.html";
    install_template_completion_project(&mut db, child_path, &source);
    let file = db.file(Utf8Path::new(child_path));

    let response = completion(&db, file, offset, PositionEncoding::Utf16, false)
        .expect("template names should complete inside quoted references");
    let items = match response {
        ls_types::CompletionResponse::Array(items) => items,
        ls_types::CompletionResponse::List(list) => list.items,
    };
    let item = items
        .iter()
        .find(|item| item.label == "account/detail.html")
        .expect("expected account/detail.html completion");
    let text_edit = item
        .text_edit
        .as_ref()
        .expect("template-name completion should carry a text edit");

    assert_eq!(
        text_edit,
        &ls_types::CompletionTextEdit::Edit(ls_types::TextEdit::new(
            ls_types::Range::new(
                ls_types::Position::new(0, 12),
                ls_types::Position::new(0, 31),
            ),
            "account/detail.html".to_string(),
        ))
    );
}

#[test]
fn template_name_completion_preserves_existing_full_close_after_open_quote() {
    let mut db = TestDatabase::new();
    let (source, offset) = source_and_offset(r#"{% extends "ba§ %}"#);
    let child_path = "/test/project/templates/child.html";
    install_template_completion_project(&mut db, child_path, &source);
    let file = db.file(Utf8Path::new(child_path));

    let response = completion(&db, file, offset, PositionEncoding::Utf16, false)
        .expect("template names should complete inside quoted references");
    let items = match response {
        ls_types::CompletionResponse::Array(items) => items,
        ls_types::CompletionResponse::List(list) => list.items,
    };
    let item = items
        .iter()
        .find(|item| item.label == "base.html")
        .expect("expected base.html completion");
    let text_edit = item
        .text_edit
        .as_ref()
        .expect("template-name completion should carry a text edit");

    assert_eq!(
        text_edit,
        &ls_types::CompletionTextEdit::Edit(ls_types::TextEdit::new(
            ls_types::Range::new(
                ls_types::Position::new(0, 12),
                ls_types::Position::new(0, 14),
            ),
            "base.html\"".to_string(),
        ))
    );
}

#[test]
fn template_name_completion_repairs_autopaired_quote_before_lone_brace() {
    let mut db = TestDatabase::new();
    let (source, offset) = source_and_offset(r#"{% include "ba§"}"#);
    let child_path = "/test/project/templates/child.html";
    install_template_completion_project(&mut db, child_path, &source);
    let file = db.file(Utf8Path::new(child_path));

    let response = completion(&db, file, offset, PositionEncoding::Utf16, false)
        .expect("template names should complete inside quoted references");
    let items = match response {
        ls_types::CompletionResponse::Array(items) => items,
        ls_types::CompletionResponse::List(list) => list.items,
    };
    let item = items
        .iter()
        .find(|item| item.label == "base.html")
        .expect("expected base.html completion");
    let text_edit = item
        .text_edit
        .as_ref()
        .expect("template-name completion should carry a text edit");

    assert_eq!(
        text_edit,
        &ls_types::CompletionTextEdit::Edit(ls_types::TextEdit::new(
            ls_types::Range::new(
                ls_types::Position::new(0, 12),
                ls_types::Position::new(0, 16),
            ),
            "base.html\" %}".to_string(),
        ))
    );
}

use camino::Utf8Path;
use djls_project::TemplateName;
use djls_semantic::SemanticOffsetContext;
use djls_source::Offset;
use djls_source::Span;
use djls_testing::TestDatabase;

fn offset_of(source: &str, needle: &str) -> Offset {
    Offset::new(u32::try_from(source.find(needle).unwrap()).unwrap())
}

fn context_for_source<'db>(
    db: &'db TestDatabase,
    source: &str,
    offset: Offset,
) -> SemanticOffsetContext<'db> {
    let path = "test.html";
    db.add_file(path, source);
    let file = db.get_or_create_file(Utf8Path::new(path));
    SemanticOffsetContext::from_offset(db, file, offset)
}

#[test]
fn identifies_template_reference_context() {
    let db = TestDatabase::new();
    let source = r#"{% extends "base.html" %}"#;

    let context = context_for_source(&db, source, offset_of(source, "base"));

    assert_eq!(
        context,
        SemanticOffsetContext::TemplateReference {
            name: TemplateName::new(&db, "base.html".to_string()),
            span: Span::saturating_from_parts_usize(11, 11),
        }
    );
}

#[test]
fn ignores_dynamic_template_reference_context() {
    let db = TestDatabase::new();
    let source = "{% include partial_name %}";

    let context = context_for_source(&db, source, offset_of(source, "partial"));

    assert_eq!(context, SemanticOffsetContext::None);
}

#[test]
fn identifies_load_library_context() {
    let db = TestDatabase::new();
    let source = "{% load static i18n %}";

    let context = context_for_source(&db, source, offset_of(source, "static"));

    assert_eq!(
        context,
        SemanticOffsetContext::LoadLibrary {
            name: "static".to_string(),
            span: Span::saturating_from_parts_usize(8, 6),
        }
    );
}

#[test]
fn identifies_selective_load_symbol_context() {
    let db = TestDatabase::new();
    let source = "{% load trans blocktrans from i18n %}";

    let context = context_for_source(&db, source, offset_of(source, "blocktrans"));

    assert_eq!(
        context,
        SemanticOffsetContext::LoadSymbol {
            name: "blocktrans".to_string(),
            span: Span::saturating_from_parts_usize(14, 10),
        }
    );
}

#[test]
fn identifies_selective_load_library_context() {
    let db = TestDatabase::new();
    let source = "{% load trans from i18n %}";

    let context = context_for_source(&db, source, offset_of(source, "i18n"));

    assert_eq!(
        context,
        SemanticOffsetContext::LoadLibrary {
            name: "i18n".to_string(),
            span: Span::saturating_from_parts_usize(19, 4),
        }
    );
}

#[test]
fn identifies_tag_name_context() {
    let db = TestDatabase::new();
    let source = "{% if user %}";

    let context = context_for_source(&db, source, offset_of(source, "if"));

    assert_eq!(
        context,
        SemanticOffsetContext::Tag {
            name: "if".to_string(),
            span: Span::saturating_from_parts_usize(3, 2),
        }
    );
}

#[test]
fn ignores_unrecognized_tag_arguments() {
    let db = TestDatabase::new();
    let source = "{% if user %}";

    let context = context_for_source(&db, source, offset_of(source, "user"));

    assert_eq!(context, SemanticOffsetContext::None);
}

#[test]
fn identifies_filter_context() {
    let db = TestDatabase::new();
    let source = "{{ user.name|title }}";

    let context = context_for_source(&db, source, offset_of(source, "title"));

    assert_eq!(
        context,
        SemanticOffsetContext::Filter {
            name: "title".to_string(),
            span: Span::new(13, 5),
        }
    );
}

#[test]
fn identifies_variable_context() {
    let db = TestDatabase::new();
    let source = "{{ user.name|title }}";

    let context = context_for_source(&db, source, offset_of(source, "user"));

    assert_eq!(
        context,
        SemanticOffsetContext::Variable {
            name: "user.name".to_string(),
            span: Span::new(3, 9),
        }
    );
}

use camino::Utf8Path;
use djls_project::TemplateName;
use djls_semantic::SemanticOffsetContext;
use djls_semantic::TemplateReferenceKind;
use djls_source::Offset;
use djls_source::Span;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;

fn offset_of(source: &str, needle: &str) -> Option<Offset> {
    let position = source.find(needle)?;
    Some(Offset::new(u32::try_from(position).ok()?))
}

fn context_for_source<'db>(
    db: &'db TestDatabase,
    source: &str,
    offset: Offset,
) -> anyhow::Result<SemanticOffsetContext<'db>> {
    let path = "test.html";
    db.add_file(path, source)?;
    let file = db.file(Utf8Path::new(path))?;
    Ok(SemanticOffsetContext::from_offset(db, file, offset))
}

#[test]
fn identifies_template_reference_context() {
    let db = TestDatabase::new();
    let source = r#"{% extends "base.html" %}"#;

    let context = context_for_source(
        &db,
        source,
        offset_of(source, "base").expect("fixture offset should resolve"),
    )
    .expect("template reference context fixture should load");

    assert_eq!(
        context,
        SemanticOffsetContext::TemplateReference {
            name: TemplateName::new(&db, "base.html".to_string()),
            kind: TemplateReferenceKind::Extends,
            span: Span::saturating_from_parts_usize(12, 9),
        }
    );
}

#[test]
fn template_reference_context_follows_load_position() {
    let mut db = TestDatabase::new();
    let source = "{% include 'before.html' %}{% load custom %}{% include 'after.html' %}";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("myproject.settings")
        .file(
            "/test/project/myproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'custom_tags'}}}]\n",
        )
        .file(
            "/test/project/custom_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag(name='include')\ndef custom_include(value):\n    pass\n",
        )
        .file("/test/project/templates/page.html", source)
        .install(&mut db)
        .expect("project fixture should install into the test database");
    db.set_project(project);
    let file = db
        .file(Utf8Path::new("/test/project/templates/page.html"))
        .expect("fixture file should exist in the test database");

    assert!(matches!(
        SemanticOffsetContext::from_offset(
            &db,
            file,
            offset_of(source, "before.html").expect("fixture offset should resolve")
        ),
        SemanticOffsetContext::TemplateReference { .. }
    ));
    assert_eq!(
        SemanticOffsetContext::from_offset(
            &db,
            file,
            offset_of(source, "after.html").expect("fixture offset should resolve")
        ),
        SemanticOffsetContext::None
    );
}

#[test]
fn identifies_template_reference_context_from_opening_quote() {
    let db = TestDatabase::new();
    let source = r#"{% extends "base.html" %}"#;

    let context = context_for_source(
        &db,
        source,
        offset_of(source, "\"base").expect("fixture offset should resolve"),
    )
    .expect("opening quote context fixture should load");

    assert_eq!(
        context,
        SemanticOffsetContext::TemplateReference {
            name: TemplateName::new(&db, "base.html".to_string()),
            kind: TemplateReferenceKind::Extends,
            span: Span::saturating_from_parts_usize(12, 9),
        }
    );
}

#[test]
fn ignores_dynamic_template_reference_context() {
    let db = TestDatabase::new();
    let source = "{% include partial_name %}";

    let context = context_for_source(
        &db,
        source,
        offset_of(source, "partial").expect("fixture offset should resolve"),
    )
    .expect("dynamic template reference context fixture should load");

    assert_eq!(context, SemanticOffsetContext::None);
}

#[test]
fn identifies_load_library_context() {
    let db = TestDatabase::new();
    let source = "{% load static i18n %}";

    let context = context_for_source(
        &db,
        source,
        offset_of(source, "static").expect("fixture offset should resolve"),
    )
    .expect("load library context fixture should load");

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

    let context = context_for_source(
        &db,
        source,
        offset_of(source, "blocktrans").expect("fixture offset should resolve"),
    )
    .expect("selective load symbol context fixture should load");

    assert_eq!(
        context,
        SemanticOffsetContext::LoadSymbol {
            name: "blocktrans".to_string(),
            library: "i18n".to_string(),
            span: Span::saturating_from_parts_usize(14, 10),
        }
    );
}

#[test]
fn identifies_selective_load_library_context() {
    let db = TestDatabase::new();
    let source = "{% load trans from i18n %}";

    let context = context_for_source(
        &db,
        source,
        offset_of(source, "i18n").expect("fixture offset should resolve"),
    )
    .expect("selective load library context fixture should load");

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

    let context = context_for_source(
        &db,
        source,
        offset_of(source, "if").expect("fixture offset should resolve"),
    )
    .expect("tag name context fixture should load");

    assert_eq!(
        context,
        SemanticOffsetContext::Tag {
            name: "if".to_string(),
            loaded_libraries: Vec::new(),
            span: Span::saturating_from_parts_usize(3, 2),
        }
    );
}

#[test]
fn identifies_template_block_name_context() {
    let db = TestDatabase::new();
    let source = "{% block content %}{% endblock %}";

    let context = context_for_source(
        &db,
        source,
        offset_of(source, "content").expect("fixture offset should resolve"),
    )
    .expect("Template Block context fixture should load");

    assert_eq!(
        context,
        SemanticOffsetContext::TemplateBlock {
            name: "content".to_string(),
            span: Span::new(9, 7),
        }
    );
}

#[test]
fn captured_intermediate_has_no_tag_definition_context() {
    let db = TestDatabase::new();
    let source = "{% if user %}{% else %}{% endif %}";

    let context = context_for_source(
        &db,
        source,
        offset_of(source, "else").expect("fixture offset should resolve"),
    )
    .expect("intermediate tag context fixture should load");

    assert_eq!(context, SemanticOffsetContext::None);
}

#[test]
fn identifies_opaque_opener_tag_context() {
    let db = TestDatabase::new();
    let source = r#"{% verbatim %}{% include "partial.html" %}{% endverbatim %}"#;

    let context = context_for_source(
        &db,
        source,
        offset_of(source, "verbatim").expect("fixture offset should resolve"),
    )
    .expect("opaque opener context fixture should load");

    assert_eq!(
        context,
        SemanticOffsetContext::Tag {
            name: "verbatim".to_string(),
            loaded_libraries: Vec::new(),
            span: Span::new(3, 8),
        }
    );
}

#[test]
fn ignores_unrecognized_tag_arguments() {
    let db = TestDatabase::new();
    let source = "{% if user %}";

    let context = context_for_source(
        &db,
        source,
        offset_of(source, "user").expect("fixture offset should resolve"),
    )
    .expect("unrecognized tag argument context fixture should load");

    assert_eq!(context, SemanticOffsetContext::None);
}

#[test]
fn ignores_template_reference_inside_verbatim() {
    let db = TestDatabase::new();
    let source = r#"{% verbatim %}{% include "partial.html" %}{% endverbatim %}"#;

    let context = context_for_source(
        &db,
        source,
        offset_of(source, "partial.html").expect("fixture offset should resolve"),
    )
    .expect("verbatim template reference context fixture should load");

    assert_eq!(context, SemanticOffsetContext::None);
}

#[test]
fn ignores_load_library_inside_comment() {
    let db = TestDatabase::new();
    let source = "{% comment %}{% load static %}{% endcomment %}";

    let context = context_for_source(
        &db,
        source,
        offset_of(source, "static").expect("fixture offset should resolve"),
    )
    .expect("commented load library context fixture should load");

    assert_eq!(context, SemanticOffsetContext::None);
}

#[test]
fn identifies_filter_context() {
    let db = TestDatabase::new();
    let source = "{{ user.name|title }}";

    let context = context_for_source(
        &db,
        source,
        offset_of(source, "title").expect("fixture offset should resolve"),
    )
    .expect("filter context fixture should load");

    assert_eq!(
        context,
        SemanticOffsetContext::Filter {
            name: "title".to_string(),
            loaded_libraries: Vec::new(),
            span: Span::new(13, 5),
        }
    );
}

#[test]
fn identifies_variable_context() {
    let db = TestDatabase::new();
    let source = "{{ user.name|title }}";

    let context = context_for_source(
        &db,
        source,
        offset_of(source, "user").expect("fixture offset should resolve"),
    )
    .expect("variable context fixture should load");

    assert_eq!(
        context,
        SemanticOffsetContext::Variable {
            name: "user.name".to_string(),
            span: Span::new(3, 9),
        }
    );
}

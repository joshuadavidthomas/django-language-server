use std::borrow::Cow;

use camino::Utf8Path;
use djls_semantic::BlockDef;
use djls_semantic::EndTag;
use djls_semantic::ExtendsTarget;
use djls_semantic::PartialDef;
use djls_semantic::TagRole;
use djls_semantic::TagSpec;
use djls_semantic::TagSpecs;
use djls_semantic::TemplateSymbols;
use djls_semantic::builtin_tag_specs;
use djls_semantic::template_symbols;
use djls_source::Span;
use djls_templates::parse_template;
use djls_testing::TestDatabase;
use rustc_hash::FxHashMap;

fn symbols_for_source<'db>(db: &'db TestDatabase, source: &str) -> &'db TemplateSymbols {
    db.add_file("test.html", source);
    let file = db.get_or_create_file(Utf8Path::new("test.html"));
    let nodelist = parse_template(db, file).expect("should parse");
    template_symbols(db, nodelist)
}

#[test]
fn extracts_partial_defs_from_partial_role_specs() {
    let mut specs = builtin_tag_specs();
    specs.merge(TagSpecs::new(FxHashMap::from_iter([(
        "partialdef".to_string(),
        TagSpec::new(
            Cow::Borrowed("django_template_partials.templatetags.partials"),
            Some(EndTag {
                name: Cow::Borrowed("endpartialdef"),
                required: true,
            }),
            Cow::Borrowed(&[]),
            false,
        )
        .with_role(TagRole::TemplatePartial),
    )])));
    let db = TestDatabase::new().with_specs(specs);
    let source = "{% partialdef card %}Body{% endpartialdef %}";
    let symbols = symbols_for_source(&db, source);

    assert!(symbols.blocks().is_empty());
    assert_eq!(
        symbols.partials(),
        &[PartialDef {
            name: "card".to_string(),
            name_span: Span::saturating_from_parts_usize(source.find("card").unwrap(), 4),
            full_span: Span::saturating_from_bounds_usize(0, source.len()),
        }]
    );
}

#[test]
fn extracts_blocks_and_extends_by_role_not_builtin_names() {
    let mut specs = builtin_tag_specs();
    specs.merge(TagSpecs::new(FxHashMap::from_iter([
        (
            "section".to_string(),
            TagSpec::new(
                Cow::Borrowed("myapp.templatetags.layout"),
                Some(EndTag {
                    name: Cow::Borrowed("endsection"),
                    required: true,
                }),
                Cow::Borrowed(&[]),
                false,
            )
            .with_role(TagRole::TemplateBlock),
        ),
        (
            "overextends".to_string(),
            TagSpec::new(
                Cow::Borrowed("myapp.templatetags.layout"),
                None,
                Cow::Borrowed(&[]),
                false,
            )
            .with_role(TagRole::TemplateReference(
                djls_semantic::TemplateReferenceKind::Extends,
            )),
        ),
    ])));
    let db = TestDatabase::new().with_specs(specs);
    let source = r#"{% overextends "base.html" %}
{% section content %}Body{% endsection %}"#;
    let symbols = symbols_for_source(&db, source);

    assert_eq!(
        symbols.extends(),
        Some(&ExtendsTarget::Literal {
            name: "base.html".to_string(),
            span: Span::saturating_from_parts_usize(source.find("base.html").unwrap(), 9),
        })
    );
    assert_eq!(
        symbols.blocks(),
        &[BlockDef {
            name: "content".to_string(),
            name_span: Span::saturating_from_parts_usize(source.find("content").unwrap(), 7),
            full_span: Span::saturating_from_bounds_usize(
                source.find("{% section").unwrap(),
                source.len(),
            ),
        }]
    );
}

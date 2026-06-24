use camino::Utf8Path;
use djls_semantic::EndTag;
use djls_semantic::IntermediateTag;
use djls_semantic::OpaqueRegions;
use djls_semantic::TagSpec;
use djls_semantic::ValidationError;
use djls_semantic::ValidationErrorAccumulator;
use djls_semantic::build_template_tree;
use djls_semantic::builtin_tag_specs;
use djls_semantic::compute_opaque_regions;
use djls_source::Span;
use djls_templates::parse_template;
use djls_testing::TestDatabase;

fn compute_regions(db: &TestDatabase, source: &str) -> OpaqueRegions {
    let path = "test.html";
    db.add_file(path, source);
    let file = db.get_or_create_file(Utf8Path::new(path));
    let nodelist = parse_template(db, file).expect("should parse");
    compute_opaque_regions(db, nodelist)
}

#[test]
fn opaque_opener_treats_intermediate_as_raw_content() {
    let mut specs = builtin_tag_specs();
    specs.insert(
        "opaque_if".to_string(),
        TagSpec::new(
            "test".into(),
            Some(EndTag {
                name: "endopaque_if".into(),
                required: true,
            }),
            vec![IntermediateTag {
                name: "opaque_else".into(),
            }]
            .into(),
            true,
        ),
    );
    let db = TestDatabase::new().with_specs(specs);
    let path = "test.html";
    let source = "{% opaque_if %}first{% opaque_else %}second{% endopaque_if %}";
    db.add_file(path, source);
    let file = db.get_or_create_file(Utf8Path::new(path));
    let nodelist = parse_template(&db, file).expect("should parse");
    let regions = compute_opaque_regions(&db, nodelist);
    let first = u32::try_from(source.find("first").unwrap()).unwrap();
    let opaque_else = u32::try_from(source.find("{% opaque_else %}").unwrap()).unwrap();
    let opaque_else_last = opaque_else + u32::try_from("{% opaque_else %}".len()).unwrap() - 1;
    let second = u32::try_from(source.find("second").unwrap()).unwrap();

    assert!(regions.is_opaque(first));
    assert!(regions.is_opaque(opaque_else));
    assert!(regions.is_opaque(opaque_else_last));
    assert!(regions.is_opaque(second));
}

#[test]
fn test_opaque_regions_empty() {
    let regions = OpaqueRegions::default();
    assert!(!regions.is_opaque(0));
    assert!(!regions.is_opaque(100));
    assert!(regions.is_empty());
}

#[test]
fn test_opaque_regions_basic() {
    let regions = OpaqueRegions::new(vec![Span::saturating_from_bounds_usize(10, 20)]);
    assert!(!regions.is_opaque(5));
    assert!(!regions.is_opaque(9));
    assert!(regions.is_opaque(10));
    assert!(regions.is_opaque(15));
    assert!(regions.is_opaque(19));
    assert!(!regions.is_opaque(20));
    assert!(!regions.is_opaque(25));
}

#[test]
fn test_opaque_regions_multiple() {
    let regions = OpaqueRegions::new(vec![
        Span::saturating_from_bounds_usize(10, 20),
        Span::saturating_from_bounds_usize(30, 40),
    ]);
    assert!(regions.is_opaque(15));
    assert!(!regions.is_opaque(25));
    assert!(regions.is_opaque(35));
    assert!(!regions.is_opaque(45));
}

#[test]
fn test_opaque_regions_sorted() {
    let regions = OpaqueRegions::new(vec![
        Span::saturating_from_bounds_usize(30, 40),
        Span::saturating_from_bounds_usize(10, 20),
    ]);
    assert!(regions.is_opaque(15));
    assert!(regions.is_opaque(35));
    assert!(!regions.is_opaque(25));
}

#[test]
fn test_verbatim_block_produces_opaque_region() {
    let db = TestDatabase::new();
    let source = "{% verbatim %}{% trans 'hello' %}{% endverbatim %}";
    let regions = compute_regions(&db, source);
    assert!(
        !regions.is_empty(),
        "verbatim block should produce an opaque region"
    );
    assert!(
        regions.is_opaque(14),
        "Position inside verbatim block should be opaque"
    );
}

#[test]
fn test_comment_block_produces_opaque_region() {
    let db = TestDatabase::new();
    let source = "{% comment %}inner content{% endcomment %}";
    let regions = compute_regions(&db, source);
    assert!(!regions.is_empty());
    assert!(regions.is_opaque(13));
}

#[test]
fn test_non_opaque_block_no_region() {
    let db = TestDatabase::new();
    let source = "{% if True %}content{% endif %}";
    let regions = compute_regions(&db, source);
    assert!(
        regions.is_empty(),
        "if block should NOT produce an opaque region"
    );
}

#[test]
fn unclosed_opaque_block_creates_no_region() {
    let db = TestDatabase::new();
    let path = "test.html";
    let source = "{% verbatim %}body";
    db.add_file(path, source);
    let file = db.get_or_create_file(Utf8Path::new(path));
    let nodelist = parse_template(&db, file).expect("should parse");
    let regions = compute_opaque_regions(&db, nodelist);
    let errors = build_template_tree::accumulated::<ValidationErrorAccumulator>(&db, nodelist);
    let body = u32::try_from(source.find("body").unwrap()).unwrap();

    assert!(!regions.is_opaque(body));
    assert!(errors.iter().any(|error| matches!(
        &error.0,
        ValidationError::UnclosedTag { tag, .. } if tag == "verbatim"
    )));
}

#[test]
fn outer_closer_inside_opaque_content_does_not_end_outer_block() {
    let db = TestDatabase::new();
    let source = "{% if outer %}{% verbatim %}{% endif %}body{% endverbatim %}{% endif %}";
    let regions = compute_regions(&db, source);
    let raw_closer = u32::try_from(source.find("{% endif %}").unwrap()).unwrap();
    let body = u32::try_from(source.find("body").unwrap()).unwrap();

    assert!(regions.is_opaque(raw_closer));
    assert!(regions.is_opaque(body));
}

#[test]
fn test_content_after_verbatim_not_opaque() {
    let db = TestDatabase::new();
    let source = "{% verbatim %}opaque{% endverbatim %}after";
    let regions = compute_regions(&db, source);
    assert!(!regions.is_opaque(37));
}

#[test]
fn test_verbatim_opaque_boundaries() {
    let db = TestDatabase::new();
    let source = "{% verbatim %}opaque{% endverbatim %}";
    let regions = compute_regions(&db, source);

    assert!(!regions.is_opaque(0), "start of opener tag");
    assert!(!regions.is_opaque(13), "end of opener tag");

    assert!(regions.is_opaque(14), "first byte of opaque content");
    assert!(regions.is_opaque(19), "last byte of opaque content");

    assert!(!regions.is_opaque(20), "start of closer tag");
    assert!(!regions.is_opaque(35), "end of closer tag");
}

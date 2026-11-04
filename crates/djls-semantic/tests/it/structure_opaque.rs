use camino::Utf8Path;
use djls_semantic::EndTag;
use djls_semantic::IntermediateTag;
use djls_semantic::TagSpec;
use djls_semantic::TemplateFold;
use djls_semantic::TemplateFoldKind;
use djls_semantic::TemplateNode;
use djls_semantic::TemplateTree;
use djls_semantic::ValidationError;
use djls_semantic::build_projectless_template_tree;
use djls_semantic::build_template_folds_for_file;
use djls_semantic::bundled_django_tag_specs;
use djls_semantic::collect_template_diagnostics;
use djls_source::Span;
use djls_templates::parse_template;
use djls_testing::ProjectFixture;
use djls_testing::ProjectSettings;
use djls_testing::TestDatabase;

fn tree_for_source<'db>(db: &'db TestDatabase, source: &str) -> anyhow::Result<TemplateTree<'db>> {
    db.add_file("test.html", source)?;
    let file = db.file(Utf8Path::new("test.html"))?;
    let nodelist = match parse_template(db, file) {
        djls_templates::TemplateParseResult::Parsed(nodelist) => nodelist,
        djls_templates::TemplateParseResult::NotTemplate => {
            anyhow::bail!("fixture file is not a template")
        }
        djls_templates::TemplateParseResult::Unreadable(error) => return Err(error.into()),
    };
    Ok(build_projectless_template_tree(db, nodelist))
}

#[test]
fn opaque_opener_treats_intermediate_as_raw_content() {
    let mut specs = bundled_django_tag_specs().clone();
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
            djls_semantic::BodyAnalysis::Opaque,
        ),
    );
    let db = TestDatabase::new().with_projectless_tag_specs(specs);
    let source = "{% opaque_if %}first{% opaque_else %}second{% endopaque_if %}";
    let tree = tree_for_source(&db, source).expect("fixture tree should build");
    let nodes = tree.regions(&db).get(tree.root(&db)).nodes();
    assert!(
        matches!(nodes, [TemplateNode::Opaque { tag, body_span, .. }]
        if tag == "opaque_if" && *body_span == Span::saturating_from_bounds_usize(15, 43))
    );

    let file = db
        .file(Utf8Path::new("test.html"))
        .expect("fixture should exist");
    let nodelist = parse_template(&db, file).expect("fixture should parse");
    assert_eq!(
        build_template_folds_for_file(&db, file, nodelist),
        &[TemplateFold {
            span: Span::saturating_from_bounds_usize(0, 61),
            kind: TemplateFoldKind::Comment,
        }]
    );
}

#[test]
fn test_opaque_regions_boundaries_and_ordering() {
    let db = TestDatabase::new();
    let source = "{% comment %}one{% endcomment %}gap{% verbatim %}two{% endverbatim %}";
    let tree = tree_for_source(&db, source).expect("fixture tree should build");
    let nodes = tree.regions(&db).get(tree.root(&db)).nodes();
    let spans = nodes
        .iter()
        .filter_map(|node| match node {
            TemplateNode::Opaque { tag, body_span, .. } => Some((tag.as_str(), *body_span)),
            TemplateNode::Block { .. }
            | TemplateNode::StandaloneTag { .. }
            | TemplateNode::Variable { .. }
            | TemplateNode::Comment { .. }
            | TemplateNode::Error { .. } => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        spans,
        [
            ("comment", Span::saturating_from_bounds_usize(13, 16)),
            ("verbatim", Span::saturating_from_bounds_usize(49, 52)),
        ]
    );
}

#[test]
fn test_comment_block_produces_opaque_region() {
    let db = TestDatabase::new();
    let tree = tree_for_source(&db, "{% comment %}inner content{% endcomment %}")
        .expect("fixture tree should build");
    let nodes = tree.regions(&db).get(tree.root(&db)).nodes();
    assert!(
        matches!(nodes, [TemplateNode::Opaque { tag, full_span, body_span, .. }]
        if tag == "comment"
            && *full_span == Span::saturating_from_bounds_usize(0, 42)
            && *body_span == Span::saturating_from_bounds_usize(13, 26))
    );
}

#[test]
fn test_non_opaque_block_no_region() {
    let db = TestDatabase::new();
    let tree =
        tree_for_source(&db, "{% if True %}content{% endif %}").expect("fixture tree should build");
    let nodes = tree.regions(&db).get(tree.root(&db)).nodes();
    assert!(matches!(nodes, [TemplateNode::Block { tag, .. }] if tag == "if"));
    assert_eq!(tree.regions(&db).iter().count(), 3);
}

#[test]
fn unclosed_opaque_block_creates_no_region() {
    let db = TestDatabase::new();
    let tree = tree_for_source(&db, "{% verbatim %}body").expect("fixture tree should build");
    let nodes = tree.regions(&db).get(tree.root(&db)).nodes();
    assert!(nodes.is_empty());
    let file = db
        .file(Utf8Path::new("test.html"))
        .expect("fixture should exist");
    let diagnostics = collect_template_diagnostics(&db, file);
    assert_eq!(
        diagnostics.validation_errors,
        [&ValidationError::UnclosedTag {
            tag: "verbatim".to_string(),
            span: Span::saturating_from_bounds_usize(0, 14),
        }]
    );
}

#[test]
fn test_verbatim_opaque_boundaries() {
    let db = TestDatabase::new();
    let tree = tree_for_source(&db, "{% verbatim %}opaque{% endverbatim %}after")
        .expect("fixture tree should build");
    let nodes = tree.regions(&db).get(tree.root(&db)).nodes();
    assert!(
        matches!(nodes, [TemplateNode::Opaque { tag, full_span, body_span, .. }]
        if tag == "verbatim"
            && *full_span == Span::saturating_from_bounds_usize(0, 37)
            && *body_span == Span::saturating_from_bounds_usize(14, 20))
    );
}

#[test]
fn validation_ignores_tags_in_opaque_bodies_but_reports_outside_tags() {
    let source = "{% missing_before %}{% comment %}{% missing_inside %}{% endcomment %}{% verbatim %}{% missing_raw %}{% endverbatim %}{% missing_after %}";
    let mut project_db = TestDatabase::new();
    ProjectFixture::new("/project")
        .settings(&ProjectSettings {
            dirs: vec!["/project/templates".to_string()],
            ..ProjectSettings::default()
        })
        .file("/project/templates/test.html", source)
        .install(&mut project_db)
        .expect("project fixture should install");
    let projectless_db = TestDatabase::new();
    projectless_db
        .add_file("test.html", source)
        .expect("fixture should be added");

    for (db, path) in [
        (&projectless_db, "test.html"),
        (&project_db, "/project/templates/test.html"),
    ] {
        let file = db.file(Utf8Path::new(path)).expect("fixture should exist");
        let diagnostics = collect_template_diagnostics(db, file);
        assert_eq!(diagnostics.template_errors.len(), 0);
        assert_eq!(
            diagnostics.validation_errors,
            [
                &ValidationError::UnknownTag {
                    tag: "missing_before".to_string(),
                    span: Span::saturating_from_bounds_usize(0, 20)
                },
                &ValidationError::UnknownTag {
                    tag: "missing_after".to_string(),
                    span: Span::saturating_from_bounds_usize(117, 136)
                },
            ]
        );
    }
}

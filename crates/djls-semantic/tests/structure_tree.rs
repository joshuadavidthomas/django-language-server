use std::borrow::Cow;

use camino::Utf8Path;
use djls_semantic::BlockRole;
use djls_semantic::EndTag;
use djls_semantic::RegionId;
use djls_semantic::TagSpec;
use djls_semantic::TagSpecs;
use djls_semantic::TemplateNode;
use djls_semantic::TemplateRegion;
use djls_semantic::TemplateTree;
use djls_semantic::ValidationError;
use djls_semantic::ValidationErrorAccumulator;
use djls_semantic::build_template_tree_for_file;
use djls_semantic::builtin_tag_specs;
use djls_source::Span;
use djls_templates::Node;
use djls_templates::TagBit;
use djls_templates::parse_template;
use djls_testing::TestDatabase;
use rustc_hash::FxHashMap;

#[derive(serde::Serialize)]
struct TemplateTreeSnapshot {
    root: u32,
    regions: Vec<RegionSnapshot>,
}

impl TemplateTreeSnapshot {
    fn from_tree(tree: TemplateTree<'_>, db: &dyn djls_semantic::Db) -> Self {
        let root = tree.root(db);
        let regions_ref = tree.regions(db);

        let regions: Vec<RegionSnapshot> = regions_ref
            .iter()
            .map(|region| RegionSnapshot {
                span: *region.span(),
                parent: region.parent().map(RegionId::id),
                nodes: region.nodes().iter().map(NodeSnapshot::from).collect(),
            })
            .collect();

        Self {
            root: root.id(),
            regions,
        }
    }
}

#[derive(serde::Serialize)]
struct RegionSnapshot {
    span: djls_source::Span,
    parent: Option<u32>,
    nodes: Vec<NodeSnapshot>,
}

#[derive(serde::Serialize)]
#[serde(tag = "node")]
enum NodeSnapshot {
    BlockTag {
        tag: String,
        name_span: djls_source::Span,
        bits: Vec<djls_templates::TagBit>,
        full_span: djls_source::Span,
        body: u32,
        role: String,
    },
    Opaque {
        tag: String,
        name_span: djls_source::Span,
        bits: Vec<djls_templates::TagBit>,
        full_span: djls_source::Span,
        body_span: djls_source::Span,
    },
    StandaloneTag {
        tag: String,
        name_span: djls_source::Span,
        bits: Vec<djls_templates::TagBit>,
        full_span: djls_source::Span,
    },
    Variable {
        var: String,
        var_span: djls_source::Span,
        filters: Vec<djls_templates::Filter>,
        span: djls_source::Span,
    },
    Comment {
        span: djls_source::Span,
    },
    Text {
        span: djls_source::Span,
    },
    Error {
        span: djls_source::Span,
        full_span: djls_source::Span,
    },
}

impl From<&TemplateNode> for NodeSnapshot {
    fn from(node: &TemplateNode) -> Self {
        match node {
            TemplateNode::Block {
                tag,
                name_span,
                bits,
                full_span,
                body,
                role,
            } => Self::BlockTag {
                tag: tag.clone(),
                name_span: *name_span,
                bits: bits.clone(),
                full_span: *full_span,
                body: body.id(),
                role: format!("{role:?}"),
            },
            TemplateNode::Opaque {
                tag,
                name_span,
                bits,
                full_span,
                body_span,
            } => Self::Opaque {
                tag: tag.clone(),
                name_span: *name_span,
                bits: bits.clone(),
                full_span: *full_span,
                body_span: *body_span,
            },
            TemplateNode::StandaloneTag {
                tag,
                name_span,
                bits,
                full_span,
            } => Self::StandaloneTag {
                tag: tag.clone(),
                name_span: *name_span,
                bits: bits.clone(),
                full_span: *full_span,
            },
            TemplateNode::Variable {
                var,
                var_span,
                filters,
                span,
            } => Self::Variable {
                var: var.clone(),
                var_span: *var_span,
                filters: filters.clone(),
                span: *span,
            },
            TemplateNode::Comment { span } => Self::Comment { span: *span },
            TemplateNode::Text { span } => Self::Text { span: *span },
            TemplateNode::Error { span, full_span } => Self::Error {
                span: *span,
                full_span: *full_span,
            },
        }
    }
}

#[derive(serde::Serialize)]
struct NodeListView {
    nodes: Vec<NodeView>,
}

#[derive(serde::Serialize)]
#[serde(tag = "kind")]
enum NodeView {
    Tag {
        name: String,
        name_span: Span,
        bits: Vec<djls_templates::TagBit>,
        span: Span,
    },
    Variable {
        var: String,
        var_span: Span,
        filters: Vec<djls_templates::Filter>,
        span: Span,
    },
    Comment {
        content: String,
        span: Span,
    },
    Text {
        span: Span,
    },
    Error {
        span: Span,
        full_span: Span,
        error: String,
    },
}

impl From<&Node> for NodeView {
    fn from(node: &Node) -> Self {
        match node {
            Node::Tag {
                name,
                name_span,
                bits,
                span,
            } => Self::Tag {
                name: name.clone(),
                name_span: *name_span,
                bits: bits.clone(),
                span: *span,
            },
            Node::Variable {
                var,
                var_span,
                filters,
                span,
            } => Self::Variable {
                var: var.clone(),
                var_span: *var_span,
                filters: filters.clone(),
                span: *span,
            },
            Node::Comment { content, span } => Self::Comment {
                content: content.clone(),
                span: *span,
            },
            Node::Text { span } => Self::Text { span: *span },
            Node::Error {
                span,
                full_span,
                error,
            } => Self::Error {
                span: *span,
                full_span: *full_span,
                error: error.to_string(),
            },
        }
    }
}

fn nodelist_view(nodes: &[Node]) -> NodeListView {
    NodeListView {
        nodes: nodes.iter().map(NodeView::from).collect(),
    }
}

#[test]
fn test_template_tree_building() {
    let db = TestDatabase::new();

    let source = r#"
{% extends "base.html" %}
{% load static i18n %}
{% block header %}
    <h1>Title</h1>
{% endblock header %}

{% if user.is_authenticated %}
    <p>Welcome {{ user.name }}</p>
    {% if user.is_superuser %}
        <span>Admin</span>
    {% elif user.is_staff %}
        <span>Manager</span>
    {% else %}
        <span>Regular user</span>
    {% endif %}
{% else %}
    <p>Please log in</p>
{% endif %}

{% for item in items %}
    <li>{{ item }}</li>
{% endfor %}
"#;

    db.add_file("test.html", source);
    let file = db.file(Utf8Path::new("test.html"));
    let nodelist = parse_template(&db, file).expect("should parse");

    insta::assert_yaml_snapshot!("nodelist", nodelist_view(nodelist.nodelist(&db)));
    let template_tree = build_template_tree_for_file(&db, file, nodelist);
    insta::assert_yaml_snapshot!(
        "template_tree",
        TemplateTreeSnapshot::from_tree(template_tree, &db)
    );
}

fn tree_for_source<'db>(db: &'db TestDatabase, source: &str) -> TemplateTree<'db> {
    db.add_file("test.html", source);
    let file = db.file(Utf8Path::new("test.html"));
    let nodelist = parse_template(db, file).expect("should parse");
    build_template_tree_for_file(db, file, nodelist)
}

fn root_region<'db>(
    tree: TemplateTree<'db>,
    db: &'db dyn djls_semantic::Db,
) -> &'db TemplateRegion {
    let root = tree.root(db);
    tree.regions(db).get(root)
}

fn first_block_body(region: &TemplateRegion, tag_name: &str) -> RegionId {
    region
        .nodes()
        .iter()
        .find_map(|node| match node {
            TemplateNode::Block { tag, body, .. } if tag == tag_name => Some(*body),
            _ => None,
        })
        .expect("expected block node")
}

fn segment_body(region: &TemplateRegion, tag_name: &str) -> RegionId {
    region
        .nodes()
        .iter()
        .find_map(|node| match node {
            TemplateNode::Block {
                tag,
                body,
                role: BlockRole::Segment,
                ..
            } if tag == tag_name => Some(*body),
            _ => None,
        })
        .expect("expected segment node")
}

fn opaque_body_span(region: &TemplateRegion, tag_name: &str) -> Span {
    region
        .nodes()
        .iter()
        .find_map(|node| match node {
            TemplateNode::Opaque { tag, body_span, .. } if tag == tag_name => Some(*body_span),
            _ => None,
        })
        .expect("expected opaque node")
}

fn assert_position_inside(span: Span, source: &str, needle: &str) {
    let position = u32::try_from(source.find(needle).expect("needle should exist")).unwrap();
    assert!(
        span.start() <= position && position < span.end(),
        "expected {needle:?} at {position} inside {span:?}"
    );
}

#[test]
fn opaque_blocks_are_opaque_nodes_without_segments() {
    let db = TestDatabase::new();
    let source = "{% verbatim %}raw{% endverbatim %}{% if user %}visible{% endif %}";
    let tree = tree_for_source(&db, source);
    let root = root_region(tree, &db);

    let body_span = opaque_body_span(root, "verbatim");
    assert_position_inside(body_span, source, "raw");
    assert!(
        !root
            .nodes()
            .iter()
            .any(|node| matches!(node, TemplateNode::Block { tag, .. } if tag == "verbatim"))
    );
    assert!(root
        .nodes()
        .iter()
        .any(|node| matches!(node, TemplateNode::Block { tag, role: BlockRole::Opener, .. } if tag == "if")));
}

#[test]
fn shared_intermediate_inside_opaque_block_has_no_structure() {
    let mut specs = builtin_tag_specs();
    specs.merge(TagSpecs::new(FxHashMap::from_iter([(
        "opaque_if".to_string(),
        TagSpec::new(
            Cow::Borrowed("test"),
            Some(EndTag {
                name: Cow::Borrowed("endopaque_if"),
                required: true,
            }),
            Cow::Owned(vec![djls_semantic::IntermediateTag {
                name: Cow::Borrowed("else"),
            }]),
            true,
        ),
    )])));
    let db = TestDatabase::new().with_projectless_tag_specs(specs);
    let source = "{% opaque_if %}{% if cond %}first{% else %}second{% endif %}{% endopaque_if %}";

    db.add_file("test.html", source);
    let file = db.file(Utf8Path::new("test.html"));
    let nodelist = parse_template(&db, file).expect("should parse");
    let tree = build_template_tree_for_file(&db, file, nodelist);
    let errors = build_template_tree_for_file::accumulated::<ValidationErrorAccumulator>(
        &db, file, nodelist,
    );

    let validation_errors = errors.iter().map(|error| &error.0).collect::<Vec<_>>();
    assert!(
        validation_errors.is_empty(),
        "shared intermediate should not be orphaned inside opaque content: {validation_errors:?}"
    );

    let root = root_region(tree, &db);
    let body_span = opaque_body_span(root, "opaque_if");
    assert_position_inside(body_span, source, "{% else %}");
    assert!(
        !tree
            .regions(&db)
            .iter()
            .flat_map(TemplateRegion::nodes)
            .any(
                |node| matches!(node, TemplateNode::StandaloneTag { tag, .. } if tag == "else")
                    || matches!(node, TemplateNode::Block { tag, .. } if tag == "else")
            )
    );
}

#[test]
fn opaque_closer_name_can_also_be_structured_opener() {
    let mut specs = builtin_tag_specs();
    specs.merge(TagSpecs::new(FxHashMap::from_iter([
        (
            "raw".to_string(),
            TagSpec::new(
                Cow::Borrowed("test"),
                Some(EndTag {
                    name: Cow::Borrowed("panel"),
                    required: true,
                }),
                Cow::Borrowed(&[]),
                true,
            ),
        ),
        (
            "panel".to_string(),
            TagSpec::new(
                Cow::Borrowed("test"),
                Some(EndTag {
                    name: Cow::Borrowed("endpanel"),
                    required: true,
                }),
                Cow::Borrowed(&[]),
                false,
            ),
        ),
    ])));
    let db = TestDatabase::new().with_projectless_tag_specs(specs);
    let source = "{% raw %}body{% panel %}";

    db.add_file("test.html", source);
    let file = db.file(Utf8Path::new("test.html"));
    let nodelist = parse_template(&db, file).expect("should parse");
    let tree = build_template_tree_for_file(&db, file, nodelist);
    let errors = build_template_tree_for_file::accumulated::<ValidationErrorAccumulator>(
        &db, file, nodelist,
    )
    .iter()
    .map(|error| &error.0)
    .collect::<Vec<_>>();

    assert!(
        errors.is_empty(),
        "opaque closer should win over colliding opener role: {errors:?}"
    );
    assert_position_inside(
        opaque_body_span(root_region(tree, &db), "raw"),
        source,
        "body",
    );

    let outside_source = "{% panel %}body{% endpanel %}";
    db.add_file("outside.html", outside_source);
    let outside_file = db.file(Utf8Path::new("outside.html"));
    let outside_nodelist = parse_template(&db, outside_file).expect("should parse");
    let outside_tree = build_template_tree_for_file(&db, outside_file, outside_nodelist);
    let outside_errors = build_template_tree_for_file::accumulated::<ValidationErrorAccumulator>(
        &db,
        outside_file,
        outside_nodelist,
    )
    .iter()
    .map(|error| &error.0)
    .collect::<Vec<_>>();

    assert!(
        outside_errors.is_empty(),
        "colliding closer should not hide opener role outside opaque content: {outside_errors:?}"
    );
    assert!(root_region(outside_tree, &db).nodes().iter().any(
        |node| matches!(node, TemplateNode::Block { tag, role: BlockRole::Opener, .. } if tag == "panel")
    ));
}

fn specs_with_standalone_structural_spellings() -> TagSpecs {
    let mut specs = builtin_tag_specs();
    specs.merge(TagSpecs::new(FxHashMap::from_iter(
        ["endif", "else", "empty"].map(|name| {
            (
                name.to_string(),
                TagSpec::new(Cow::Borrowed("test"), None, Cow::Borrowed(&[]), false),
            )
        }),
    )));
    specs
}

#[test]
fn standalone_definitions_win_over_top_level_structural_vocabulary() {
    let db = TestDatabase::new()
        .with_projectless_tag_specs(specs_with_standalone_structural_spellings());
    let source = "{% endif %}{% else %}{% empty %}";

    db.add_file("test.html", source);
    let file = db.file(Utf8Path::new("test.html"));
    let nodelist = parse_template(&db, file).expect("should parse");
    let tree = build_template_tree_for_file(&db, file, nodelist);
    let errors = build_template_tree_for_file::accumulated::<ValidationErrorAccumulator>(
        &db, file, nodelist,
    )
    .iter()
    .map(|error| &error.0)
    .collect::<Vec<_>>();

    assert!(
        errors.is_empty(),
        "effective standalone definitions must not be orphaned: {errors:?}"
    );
    let standalone_tags = root_region(tree, &db)
        .nodes()
        .iter()
        .filter_map(|node| match node {
            TemplateNode::StandaloneTag { tag, .. } => Some(tag.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(standalone_tags, ["endif", "else", "empty"]);
}

#[test]
fn matching_branch_context_wins_but_other_collisions_stay_standalone_in_blocks() {
    let db = TestDatabase::new()
        .with_projectless_tag_specs(specs_with_standalone_structural_spellings());
    let source = "{% if condition %}{% empty %}{% else %}after{% endif %}";

    db.add_file("test.html", source);
    let file = db.file(Utf8Path::new("test.html"));
    let nodelist = parse_template(&db, file).expect("should parse");
    let tree = build_template_tree_for_file(&db, file, nodelist);
    let errors = build_template_tree_for_file::accumulated::<ValidationErrorAccumulator>(
        &db, file, nodelist,
    )
    .iter()
    .map(|error| &error.0)
    .collect::<Vec<_>>();

    assert!(
        errors.is_empty(),
        "matching if context should consume else/endif without consuming empty: {errors:?}"
    );
    let if_container = first_block_body(root_region(tree, &db), "if");
    let initial_segment = segment_body(tree.regions(&db).get(if_container), "if");
    assert!(
        tree.regions(&db)
            .get(initial_segment)
            .nodes()
            .iter()
            .any(|node| matches!(node, TemplateNode::StandaloneTag { tag, .. } if tag == "empty"))
    );
    assert!(tree.regions(&db).get(if_container).nodes().iter().any(
        |node| matches!(node, TemplateNode::Block { tag, role: BlockRole::Segment, .. } if tag == "else")
    ));
    assert!(!tree.regions(&db).iter().flat_map(TemplateRegion::nodes).any(
        |node| matches!(node, TemplateNode::StandaloneTag { tag, .. } if tag == "else" || tag == "endif")
    ));
}

#[test]
fn unclosed_optional_opaque_block_reports_unclosed_without_node() {
    let mut specs = builtin_tag_specs();
    specs.merge(TagSpecs::new(FxHashMap::from_iter([(
        "raw".to_string(),
        TagSpec::new(
            Cow::Borrowed("test"),
            Some(EndTag {
                name: Cow::Borrowed("endraw"),
                required: false,
            }),
            Cow::Borrowed(&[]),
            true,
        ),
    )])));
    let db = TestDatabase::new().with_projectless_tag_specs(specs);
    let source = "{% raw %}body";

    db.add_file("test.html", source);
    let file = db.file(Utf8Path::new("test.html"));
    let nodelist = parse_template(&db, file).expect("should parse");
    let tree = build_template_tree_for_file(&db, file, nodelist);
    let errors = build_template_tree_for_file::accumulated::<ValidationErrorAccumulator>(
        &db, file, nodelist,
    )
    .iter()
    .map(|error| &error.0)
    .collect::<Vec<_>>();

    assert!(
        errors
            .iter()
            .any(|error| matches!(error, ValidationError::UnclosedTag { tag, .. } if tag == "raw"))
    );
    assert!(
        !tree
            .regions(&db)
            .iter()
            .flat_map(TemplateRegion::nodes)
            .any(|node| matches!(node, TemplateNode::Opaque { tag, .. } if tag == "raw"))
    );
}

#[test]
fn known_opener_and_closer_inside_opaque_block_have_no_structure() {
    let db = TestDatabase::new();
    let source = "{% verbatim %}{% if x %}body{% endif %}{% endverbatim %}";

    db.add_file("test.html", source);
    let file = db.file(Utf8Path::new("test.html"));
    let nodelist = parse_template(&db, file).expect("should parse");
    let tree = build_template_tree_for_file(&db, file, nodelist);
    let errors = build_template_tree_for_file::accumulated::<ValidationErrorAccumulator>(
        &db, file, nodelist,
    )
    .iter()
    .map(|error| &error.0)
    .collect::<Vec<_>>();

    assert!(
        errors.is_empty(),
        "known tags inside opaque content should not affect structure: {errors:?}"
    );

    let body_span = opaque_body_span(root_region(tree, &db), "verbatim");
    assert_position_inside(body_span, source, "{% if x %}");
    assert_position_inside(body_span, source, "{% endif %}");
    assert!(!tree.regions(&db).iter().flat_map(TemplateRegion::nodes).any(
        |node| matches!(node, TemplateNode::StandaloneTag { tag, .. } if tag == "if" || tag == "endif")
            || matches!(node, TemplateNode::Block { tag, .. } if tag == "if")
    ));
}

#[test]
fn outer_closer_inside_opaque_block_has_no_structure() {
    let db = TestDatabase::new();
    let source = "{% if outer %}{% verbatim %}{% endif %}{% endverbatim %}{% endif %}";

    db.add_file("test.html", source);
    let file = db.file(Utf8Path::new("test.html"));
    let nodelist = parse_template(&db, file).expect("should parse");
    let tree = build_template_tree_for_file(&db, file, nodelist);
    let errors = build_template_tree_for_file::accumulated::<ValidationErrorAccumulator>(
        &db, file, nodelist,
    )
    .iter()
    .map(|error| &error.0)
    .collect::<Vec<_>>();

    assert!(
        errors.is_empty(),
        "outer closer inside opaque content should be raw content: {errors:?}"
    );

    let if_container = first_block_body(root_region(tree, &db), "if");
    let if_body = segment_body(tree.regions(&db).get(if_container), "if");
    let body_span = opaque_body_span(tree.regions(&db).get(if_body), "verbatim");
    assert_position_inside(body_span, source, "{% endif %}");
}

#[test]
fn top_level_standalone_tags_are_visible() {
    let db = TestDatabase::new();
    let tree = tree_for_source(
        &db,
        r#"{% extends "base.html" %}
{% load static i18n %}
{% include "partials/nav.html" %}"#,
    );

    let tags = root_region(tree, &db)
        .nodes()
        .iter()
        .filter_map(|node| match node {
            TemplateNode::StandaloneTag { tag, bits, .. } => Some((
                tag.as_str(),
                bits.iter().map(TagBit::as_str).collect::<Vec<_>>(),
            )),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(tags.len(), 3);
    assert_eq!(tags[0], ("extends", vec!["\"base.html\""]));
    assert_eq!(tags[1], ("load", vec!["static", "i18n"]));
    assert_eq!(tags[2], ("include", vec!["\"partials/nav.html\""]));
}

#[test]
fn nested_blocks_preserve_hierarchy() {
    let db = TestDatabase::new();
    let tree = tree_for_source(
        &db,
        r"{% block content %}
  {% block title %}Title{% endblock %}
{% endblock %}",
    );

    let root = root_region(tree, &db);
    let content_container = first_block_body(root, "block");
    let content_body = segment_body(tree.regions(&db).get(content_container), "block");
    let title_container = first_block_body(tree.regions(&db).get(content_body), "block");
    let title_body = segment_body(tree.regions(&db).get(title_container), "block");

    assert!(
        tree.regions(&db)
            .get(title_body)
            .nodes()
            .iter()
            .any(|node| matches!(node, TemplateNode::Text { .. }))
    );
}

#[test]
fn intermediate_tags_preserve_segments() {
    let db = TestDatabase::new();
    let tree = tree_for_source(
        &db,
        r"{% if user %}
  Hello
{% elif staff %}
  Staff
{% else %}
  Anonymous
{% endif %}",
    );

    let if_container = first_block_body(root_region(tree, &db), "if");
    let segment_tags = tree
        .regions(&db)
        .get(if_container)
        .nodes()
        .iter()
        .filter_map(|node| match node {
            TemplateNode::Block {
                tag,
                bits,
                role: BlockRole::Segment,
                ..
            } => Some((
                tag.as_str(),
                bits.iter().map(TagBit::as_str).collect::<Vec<_>>(),
            )),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(segment_tags.len(), 3);
    assert_eq!(segment_tags[0], ("if", vec!["user"]));
    assert_eq!(segment_tags[1], ("elif", vec!["staff"]));
    assert_eq!(segment_tags[2], ("else", Vec::<&str>::new()));
}

#[test]
fn standalone_tags_inside_blocks_attach_to_block_region() {
    let db = TestDatabase::new();
    let tree = tree_for_source(
        &db,
        r#"{% block content %}
  {% include "card.html" %}
{% endblock %}"#,
    );

    let content_container = first_block_body(root_region(tree, &db), "block");
    let content_body = segment_body(tree.regions(&db).get(content_container), "block");
    assert!(
        tree.regions(&db)
            .get(content_body)
            .nodes()
            .iter()
            .any(|node| matches!(
                node,
                TemplateNode::StandaloneTag { tag, bits, .. }
                    if tag == "include"
                        && bits.first().is_some_and(|arg| arg.as_str() == "\"card.html\"")
            ))
    );
}

#[test]
fn malformed_recovery_is_best_effort() {
    let db = TestDatabase::new();
    let source = r"{% block content %}
  {% if user %}
{% endblock %}";

    db.add_file("test.html", source);
    let file = db.file(Utf8Path::new("test.html"));
    let nodelist = parse_template(&db, file).expect("should parse");
    let tree = build_template_tree_for_file(&db, file, nodelist);
    let errors = build_template_tree_for_file::accumulated::<ValidationErrorAccumulator>(
        &db, file, nodelist,
    );

    assert!(
        root_region(tree, &db)
            .nodes()
            .iter()
            .any(|node| matches!(node, TemplateNode::Block { tag, .. } if tag == "block"))
    );
    assert!(errors.iter().any(
        |error| matches!(error.0, ValidationError::UnclosedTag { ref tag, .. } if tag == "if")
    ));
}

#[test]
fn custom_block_tags_from_specs_are_blocks() {
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
        ),
    )])));
    let db = TestDatabase::new().with_projectless_tag_specs(specs);
    let tree = tree_for_source(&db, "{% partialdef card %}Body{% endpartialdef %}");

    assert!(root_region(tree, &db).nodes().iter().any(|node| matches!(
        node,
        TemplateNode::Block { tag, bits, .. }
            if tag == "partialdef"
                && bits.first().is_some_and(|arg| arg.as_str() == "card")
    )));
}

#[test]
fn test_endblock_name_mismatch() {
    let db = TestDatabase::new();

    let source = r"
{% block content %}
    <p>Hello</p>
{% endblock fdsaf %}
";

    db.add_file("test.html", source);
    let file = db.file(Utf8Path::new("test.html"));
    let nodelist = parse_template(&db, file).expect("should parse");
    let errors = build_template_tree_for_file::accumulated::<ValidationErrorAccumulator>(
        &db, file, nodelist,
    );
    assert_eq!(errors.len(), 1);
    let opener_start = source
        .find("{% block content %}")
        .expect("fixture should contain opener");
    let expected_opener_span = Span::saturating_from_parts_usize(opener_start, 19);
    let got_start = source
        .find("fdsaf")
        .expect("fixture should contain closer name");
    let expected_got_span = Span::saturating_from_parts_usize(got_start, 5);
    assert!(
        matches!(
            &errors[0].0,
            ValidationError::UnmatchedBlockName { expected, got, got_span, opener_span, .. }
                if expected == "content"
                    && got == "fdsaf"
                    && *got_span == expected_got_span
                    && *opener_span == expected_opener_span
        ),
        "Expected UnmatchedBlockName, got: {:?}",
        errors[0].0
    );
}

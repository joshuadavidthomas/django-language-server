use djls_source::Span;
use djls_templates::Filter;
use djls_templates::TagArgument;
use serde::Serialize;

#[salsa::tracked]
pub struct TemplateTree<'db> {
    pub root: RegionId,
    #[returns(ref)]
    pub regions: Regions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct RegionId(u32);

impl RegionId {
    #[must_use]
    pub fn new(id: u32) -> Self {
        Self(id)
    }

    #[must_use]
    pub fn id(self) -> u32 {
        self.0
    }

    #[must_use]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct Regions(Vec<TemplateRegion>);

impl Regions {
    #[must_use]
    pub fn get(&self, id: RegionId) -> &TemplateRegion {
        &self[id]
    }

    pub fn iter(&self) -> std::slice::Iter<'_, TemplateRegion> {
        self.0.iter()
    }

    /// Allocate a new region in the template tree.
    ///
    /// # Panics
    ///
    /// Panics if the number of regions exceeds `u32::MAX`.
    pub(crate) fn alloc(&mut self, span: Span, parent: Option<RegionId>) -> RegionId {
        let next = self.0.len();
        let id = u32::try_from(next).expect("too many regions (overflow u32::MAX)");
        self.0.push(TemplateRegion::new(span, parent));
        RegionId(id)
    }

    pub(crate) fn extend_region(&mut self, id: RegionId, span: Span) {
        self.region_mut(id).extend_span(span);
    }

    pub(crate) fn finalize_region_span(&mut self, id: RegionId, end: u32) {
        let region = self.region_mut(id);
        let start = region.span().start();
        region.set_span(Span::saturating_from_bounds_usize(
            start as usize,
            end as usize,
        ));
    }

    pub(crate) fn push_node(&mut self, target: RegionId, node: TemplateNode) {
        let span = node.span();
        self.extend_region(target, span);
        self.region_mut(target).nodes.push(node);
    }

    fn region_mut(&mut self, id: RegionId) -> &mut TemplateRegion {
        let idx = id.index();
        &mut self.0[idx]
    }
}

impl std::ops::Index<RegionId> for Regions {
    type Output = TemplateRegion;

    fn index(&self, id: RegionId) -> &Self::Output {
        &self.0[id.index()]
    }
}

impl<'a> IntoIterator for &'a Regions {
    type Item = &'a TemplateRegion;
    type IntoIter = std::slice::Iter<'a, TemplateRegion>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct TemplateRegion {
    span: Span,
    nodes: Vec<TemplateNode>,
    parent: Option<RegionId>,
}

impl TemplateRegion {
    fn new(span: Span, parent: Option<RegionId>) -> Self {
        Self {
            span,
            nodes: Vec::new(),
            parent,
        }
    }

    #[must_use]
    pub fn span(&self) -> &Span {
        &self.span
    }

    #[must_use]
    pub fn nodes(&self) -> &[TemplateNode] {
        &self.nodes
    }

    #[must_use]
    pub fn parent(&self) -> Option<RegionId> {
        self.parent
    }

    pub(crate) fn set_span(&mut self, span: Span) {
        self.span = span;
    }

    fn extend_span(&mut self, span: Span) {
        let opening = self.span.start().saturating_sub(span.start());
        let closing = span.end().saturating_sub(self.span.end());
        self.span = self.span.expand(opening, closing);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum BlockRole {
    /// A block tag attached to its parent region. Its body points to the
    /// container region that owns the block's segments.
    Opener,
    /// A block segment attached to a block container region. Its body points to
    /// the content region for that segment.
    Segment,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum TemplateNode {
    /// A structural block node.
    ///
    /// Blocks are represented in two arena hops: an `Opener` node appears in
    /// the parent content region and points to a container region; that
    /// container owns one or more `Segment` nodes, each pointing to its content
    /// region. This keeps intermediate tags like `elif`/`else` in source order
    /// without nested ownership inside the Salsa-tracked tree.
    Block {
        tag: String,
        tag_span: Span,
        arguments: Vec<TagArgument>,
        marker_span: Span,
        full_span: Span,
        body: RegionId,
        role: BlockRole,
    },
    StandaloneTag {
        tag: String,
        tag_span: Span,
        arguments: Vec<TagArgument>,
        marker_span: Span,
        full_span: Span,
    },
    Variable {
        var: String,
        var_span: Span,
        filters: Vec<Filter>,
        span: Span,
    },
    Comment {
        span: Span,
    },
    Text {
        span: Span,
    },
    Error {
        span: Span,
        full_span: Span,
    },
}

impl TemplateNode {
    fn span(&self) -> Span {
        match self {
            TemplateNode::Block { full_span, .. }
            | TemplateNode::StandaloneTag { full_span, .. }
            | TemplateNode::Error { full_span, .. } => *full_span,
            TemplateNode::Variable { span, .. }
            | TemplateNode::Comment { span }
            | TemplateNode::Text { span } => *span,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use djls_source::File;
    use djls_source::Span;
    use djls_templates::parse_template;
    use djls_templates::Node;
    use djls_templates::TagArgument;
    use rustc_hash::FxHashMap;

    use super::BlockRole;
    use super::RegionId;
    use super::TemplateNode;
    use super::TemplateRegion;
    use super::TemplateTree;
    use crate::build_template_tree;
    use crate::builtin_tag_specs;
    use crate::structure::snapshot::TemplateTreeSnapshot;
    use crate::testing::TestDatabase;
    use crate::EndTag;
    use crate::TagSpec;
    use crate::TagSpecs;
    use crate::ValidationError;

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
            arguments: Vec<djls_templates::TagArgument>,
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
                    arguments,
                    span,
                } => Self::Tag {
                    name: name.clone(),
                    name_span: *name_span,
                    arguments: arguments.clone(),
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
        let file = File::new(&db, "test.html".into(), 0);
        let nodelist = parse_template(&db, file).expect("should parse");

        insta::assert_yaml_snapshot!("nodelist", nodelist_view(nodelist.nodelist(&db)));
        let template_tree = build_template_tree(&db, nodelist);
        insta::assert_yaml_snapshot!(
            "template_tree",
            TemplateTreeSnapshot::from_tree(template_tree, &db)
        );
    }

    fn tree_for_source<'db>(db: &'db TestDatabase, source: &str) -> TemplateTree<'db> {
        db.add_file("test.html", source);
        let file = File::new(db, "test.html".into(), 0);
        let nodelist = parse_template(db, file).expect("should parse");
        build_template_tree(db, nodelist)
    }

    fn root_region<'db>(tree: TemplateTree<'db>, db: &'db dyn crate::Db) -> &'db TemplateRegion {
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
                TemplateNode::StandaloneTag { tag, arguments, .. } => Some((
                    tag.as_str(),
                    arguments
                        .iter()
                        .map(TagArgument::as_str)
                        .collect::<Vec<_>>(),
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

        assert!(tree
            .regions(&db)
            .get(title_body)
            .nodes()
            .iter()
            .any(|node| matches!(node, TemplateNode::Text { .. })));
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
                    arguments,
                    role: BlockRole::Segment,
                    ..
                } => Some((
                    tag.as_str(),
                    arguments
                        .iter()
                        .map(TagArgument::as_str)
                        .collect::<Vec<_>>(),
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
        assert!(tree
            .regions(&db)
            .get(content_body)
            .nodes()
            .iter()
            .any(|node| matches!(
                node,
                TemplateNode::StandaloneTag { tag, arguments, .. }
                    if tag == "include"
                        && arguments.first().is_some_and(|arg| arg.as_str() == "\"card.html\"")
            )));
    }

    #[test]
    fn malformed_recovery_is_best_effort() {
        let db = TestDatabase::new();
        let source = r"{% block content %}
  {% if user %}
{% endblock %}";

        db.add_file("test.html", source);
        let file = File::new(&db, "test.html".into(), 0);
        let nodelist = parse_template(&db, file).expect("should parse");
        let tree = build_template_tree(&db, nodelist);
        let errors =
            build_template_tree::accumulated::<crate::ValidationErrorAccumulator>(&db, nodelist);

        assert!(root_region(tree, &db)
            .nodes()
            .iter()
            .any(|node| matches!(node, TemplateNode::Block { tag, .. } if tag == "block")));
        assert!(errors.iter().any(
            |error| matches!(error.0, ValidationError::UnclosedTag { ref tag, .. } if tag == "if")
        ));
    }

    #[test]
    fn custom_block_tags_from_specs_are_blocks() {
        let mut specs = builtin_tag_specs();
        specs.merge(TagSpecs::new(FxHashMap::from_iter([(
            "partialdef".to_string(),
            TagSpec {
                module: Cow::Borrowed("django_template_partials.templatetags.partials"),
                end_tag: Some(EndTag {
                    name: Cow::Borrowed("endpartialdef"),
                    required: true,
                }),
                intermediate_tags: Cow::Borrowed(&[]),
                opaque: false,
                outline_role: None,
                extracted_rules: None,
            },
        )])));
        let db = TestDatabase::new().with_specs(specs);
        let tree = tree_for_source(&db, "{% partialdef card %}Body{% endpartialdef %}");

        assert!(root_region(tree, &db).nodes().iter().any(|node| matches!(
            node,
            TemplateNode::Block { tag, arguments, .. }
                if tag == "partialdef"
                    && arguments.first().is_some_and(|arg| arg.as_str() == "card")
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
        let file = File::new(&db, "test.html".into(), 0);
        let nodelist = parse_template(&db, file).expect("should parse");
        let errors =
            build_template_tree::accumulated::<crate::ValidationErrorAccumulator>(&db, nodelist);
        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                &errors[0].0,
                crate::ValidationError::UnmatchedBlockName { expected, got, .. }
                    if expected == "content" && got == "fdsaf"
            ),
            "Expected UnmatchedBlockName, got: {:?}",
            errors[0].0
        );
    }
}

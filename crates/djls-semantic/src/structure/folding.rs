use djls_source::Span;
use djls_templates::Node;
use serde::Serialize;

use crate::structure::build_block_tree;
use crate::structure::BlockId;
use crate::structure::BlockNode;
use crate::structure::Blocks;
use crate::structure::BranchKind;
use crate::Db;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum TemplateFoldKind {
    Region,
    Comment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct TemplateFold {
    pub span: Span,
    pub kind: TemplateFoldKind,
}

impl TemplateFold {
    fn region(span: Span) -> Self {
        Self {
            span,
            kind: TemplateFoldKind::Region,
        }
    }

    fn comment(span: Span) -> Self {
        Self {
            span,
            kind: TemplateFoldKind::Comment,
        }
    }
}

#[salsa::tracked]
pub fn collect_template_folds<'db>(
    db: &'db dyn Db,
    nodelist: djls_templates::NodeList<'db>,
) -> Vec<TemplateFold> {
    let tree = build_block_tree(db, nodelist);
    let blocks = tree.blocks(db);
    let mut folds = Vec::new();

    for root in tree.roots(db) {
        collect_container_folds(*root, blocks, &mut folds);
    }

    for node in nodelist.nodelist(db) {
        if let Node::Comment { .. } = node {
            folds.push(TemplateFold::comment(node.full_span()));
        }
    }

    folds.sort_by_key(|fold| (fold.span.start(), fold.span.end(), fold.kind_key()));
    folds.dedup();
    folds
}

fn collect_container_folds(container_id: BlockId, blocks: &Blocks, folds: &mut Vec<TemplateFold>) {
    let container = blocks.get(container_id.index());
    folds.push(TemplateFold::region(*container.span()));

    for node in container.nodes() {
        if let BlockNode::Branch {
            body,
            kind: BranchKind::Segment,
            ..
        } = node
        {
            collect_body_folds(*body, blocks, folds);
        }
    }
}

fn collect_body_folds(body_id: BlockId, blocks: &Blocks, folds: &mut Vec<TemplateFold>) {
    let body = blocks.get(body_id.index());

    for node in body.nodes() {
        match node {
            BlockNode::Branch {
                body: container_id,
                kind: BranchKind::Opener,
                ..
            } => collect_container_folds(*container_id, blocks, folds),
            BlockNode::Branch {
                body: segment_id,
                kind: BranchKind::Segment,
                ..
            } => collect_body_folds(*segment_id, blocks, folds),
            BlockNode::Leaf { .. } => {}
        }
    }
}

impl TemplateFold {
    fn kind_key(self) -> u8 {
        match self.kind {
            TemplateFoldKind::Region => 0,
            TemplateFoldKind::Comment => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use djls_source::File;
    use djls_templates::parse_template;
    use insta::assert_yaml_snapshot;

    use super::*;
    use crate::testing::TestDatabase;

    #[test]
    fn collects_nested_template_folds() {
        let db = TestDatabase::new();
        let source = r"
{% block content %}
    {#
    folded comment
    #}
    {% if user.is_authenticated %}
        <p>Hello</p>
    {% else %}
        <p>Goodbye</p>
    {% endif %}
{% endblock content %}
";

        db.add_file("template.html", source);
        let file = File::new(&db, "template.html".into(), 0);
        let nodelist = parse_template(&db, file).expect("should parse");
        let folds = collect_template_folds(&db, nodelist);

        assert_yaml_snapshot!(folds);
    }
}

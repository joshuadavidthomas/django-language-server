use djls_source::Span;
use djls_templates::nodelist::TagBit;
use djls_templates::nodelist::TagName;
use djls_templates::Node;
use djls_templates::NodeList;
use serde::Serialize;

use super::shapes::CloseValidation;
use super::shapes::TagClass;
use super::shapes::TagShape;
use super::shapes::TagShapes;
use crate::db::Db;

#[derive(Debug, Serialize)]
pub struct BlockTree {
    roots: Vec<BlockId>,
    blocks: Blocks,
}

/// Context for building a BlockTree - holds all the ambient state
struct BuildContext<'db> {
    db: &'db dyn Db,
    shapes: &'db TagShapes,
    root_id: BlockId,
    stack: Vec<TreeFrame<'db>>,
}

impl<'db> BuildContext<'db> {
    fn new(db: &'db dyn Db, shapes: &'db TagShapes, root_id: BlockId) -> Self {
        Self {
            db,
            shapes,
            root_id,
            stack: Vec::new(),
        }
    }

    fn active_segment(&self) -> BlockId {
        self.stack
            .last()
            .map_or(self.root_id, |frame| frame.segment_body)
    }

    fn find_frame(&self, opener_name: &str) -> Option<usize> {
        self.stack
            .iter()
            .rposition(|f| f.opener_name == opener_name)
    }
}

impl BlockTree {
    pub fn new() -> Self {
        let (blocks, root) = Blocks::with_root();
        Self {
            roots: vec![root],
            blocks,
        }
    }

    /// Build the tree from a nodelist
    pub fn build(db: &dyn Db, nodelist: NodeList, shapes: &TagShapes) -> Self {
        let mut tree = BlockTree::new();
        let root_id = tree.roots[0];

        let mut ctx = BuildContext::new(db, shapes, root_id);

        for node in nodelist.nodelist(db).iter().cloned() {
            tree.handle_node(&mut ctx, node);
        }

        tree.finish(&mut ctx);
        tree
    }

    fn handle_node<'db>(&mut self, ctx: &mut BuildContext<'db>, node: Node<'db>) {
        match node {
            Node::Tag { name, bits, span } => {
                self.handle_tag(ctx, name, bits, span);
            }
            Node::Comment { span, .. } => {
                self.blocks
                    .add_leaf(ctx.active_segment(), "<comment>".into(), span);
            }
            Node::Variable { span, .. } => {
                self.blocks
                    .add_leaf(ctx.active_segment(), "<var>".into(), span);
            }
            Node::Text { .. } => {
                // Skip text nodes - we only care about Django constructs
            }
            Node::Error {
                full_span, error, ..
            } => {
                self.blocks
                    .add_leaf(ctx.active_segment(), error.to_string(), full_span);
            }
        }
    }

    fn handle_tag<'db>(
        &mut self,
        ctx: &mut BuildContext<'db>,
        name: TagName<'db>,
        bits: Vec<TagBit<'db>>,
        span: Span,
    ) {
        let tag_name = name.text(ctx.db);

        match ctx.shapes.classify(&tag_name) {
            TagClass::Opener { .. } => {
                let parent = ctx.active_segment();
                let (container, segment) = self.blocks.add_block(parent, &tag_name, span);

                ctx.stack.push(TreeFrame {
                    opener_name: tag_name,
                    opener_bits: bits,
                    opener_span: span,
                    container_body: container,
                    segment_body: segment,
                    parent_body: parent,
                });
            }

            TagClass::Closer { opener_name } => {
                self.close_block(ctx, &opener_name, &bits, span);
            }

            TagClass::Intermediate { possible_openers } => {
                self.add_intermediate(ctx, &tag_name, &possible_openers, span);
            }

            TagClass::Unknown => {
                // Treat as leaf
                self.blocks.add_leaf(ctx.active_segment(), tag_name, span);
            }
        }
    }

    fn close_block<'db>(
        &mut self,
        ctx: &mut BuildContext<'db>,
        opener_name: &str,
        closer_bits: &[TagBit<'db>],
        span: Span,
    ) {
        // Find the matching frame
        if let Some(frame_idx) = ctx.find_frame(opener_name) {
            // Pop any unclosed blocks above this one
            while ctx.stack.len() > frame_idx + 1 {
                if let Some(unclosed) = ctx.stack.pop() {
                    self.blocks.add_error(
                        unclosed.parent_body,
                        format!("Unclosed block '{}'", unclosed.opener_name),
                        unclosed.opener_span,
                    );
                }
            }

            // Now validate and close
            let frame = ctx.stack.pop().unwrap();
            match ctx
                .shapes
                .validate_close(opener_name, &frame.opener_bits, closer_bits, ctx.db)
            {
                CloseValidation::Valid => {
                    self.blocks.extend(frame.container_body, span);
                }
                CloseValidation::ArgumentMismatch { arg, expected, got } => {
                    self.blocks.add_error(
                        frame.segment_body,
                        format!("Argument '{arg}' mismatch: expected '{expected}', got '{got}'",),
                        span,
                    );
                    ctx.stack.push(frame); // Restore frame
                }
                CloseValidation::MissingRequiredArg { arg, expected } => {
                    self.blocks.add_error(
                        frame.segment_body,
                        format!("Missing required argument '{arg}': expected '{expected}'",),
                        span,
                    );
                    ctx.stack.push(frame);
                }
                CloseValidation::UnexpectedArg { arg, got } => {
                    self.blocks.add_error(
                        frame.segment_body,
                        format!("Unexpected argument '{arg}' with value '{got}'"),
                        span,
                    );
                    ctx.stack.push(frame);
                }
                CloseValidation::NotABlock => {
                    // Should not happen as we already classified it
                    self.blocks.add_error(
                        ctx.active_segment(),
                        format!("Internal error: {opener_name} is not a block"),
                        span,
                    );
                }
            }
        } else {
            self.blocks.add_error(
                ctx.active_segment(),
                format!("Unexpected closing tag '{opener_name}'"),
                span,
            );
        }
    }

    fn add_intermediate(
        &mut self,
        ctx: &mut BuildContext<'_>,
        tag_name: &str,
        possible_openers: &[String],
        span: Span,
    ) {
        if let Some(frame) = ctx.stack.last_mut() {
            if possible_openers.contains(&frame.opener_name) {
                // Add new segment
                frame.segment_body =
                    self.blocks
                        .add_segment(frame.container_body, tag_name.to_string(), span);
            } else {
                self.blocks.add_error(
                    frame.segment_body,
                    format!("'{}' is not valid in '{}'", tag_name, frame.opener_name),
                    span,
                );
            }
        } else {
            self.blocks.add_error(
                ctx.root_id,
                format!("Intermediate tag '{tag_name}' outside of block"),
                span,
            );
        }
    }

    fn finish(&mut self, ctx: &mut BuildContext<'_>) {
        // Close any remaining open blocks
        while let Some(frame) = ctx.stack.pop() {
            // Check if this block's end tag was optional
            if let Some(TagShape::Block { end, .. }) = ctx.shapes.get(&frame.opener_name) {
                if end.optional {
                    self.blocks.extend(frame.container_body, frame.opener_span);
                } else {
                    self.blocks.add_error(
                        frame.parent_body,
                        format!("Unclosed block '{}'", frame.opener_name),
                        frame.opener_span,
                    );
                }
            }
        }
    }
}

impl Default for BlockTree {
    fn default() -> Self {
        Self::new()
    }
}

struct TreeFrame<'db> {
    opener_name: String,
    opener_bits: Vec<TagBit<'db>>,
    opener_span: Span,
    container_body: BlockId,
    segment_body: BlockId,
    parent_body: BlockId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct BlockId(u32);

impl BlockId {
    fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Default, Serialize)]
pub struct Blocks(Vec<Block>);

impl Blocks {
    fn with_root() -> (Self, BlockId) {
        let mut blocks = Self::default();
        let root = blocks.alloc(Span::new(0, 0));
        (blocks, root)
    }

    fn alloc(&mut self, span: Span) -> BlockId {
        let id = BlockId(self.0.len() as u32);
        self.0.push(Block::new(span));
        id
    }

    fn add_block(&mut self, parent: BlockId, name: &str, span: Span) -> (BlockId, BlockId) {
        let container = self.alloc(span);

        self.push_node(
            parent,
            BlockNode::Block {
                name: name.to_string(),
                span,
                body: container,
            },
        );
        let segment = self.add_segment(container, name.to_string(), span);

        (container, segment)
    }

    fn add_segment(&mut self, container: BlockId, label: String, span: Span) -> BlockId {
        let segment = self.alloc(span);
        self.push_node(
            container,
            BlockNode::Segment {
                label,
                span,
                body: segment,
            },
        );
        segment
    }

    fn add_leaf(&mut self, target: BlockId, label: String, span: Span) {
        self.push_node(target, BlockNode::Leaf { label, span });
    }

    fn add_error(&mut self, target: BlockId, message: String, span: Span) {
        self.push_node(target, BlockNode::Error { message, span });
    }

    fn extend(&mut self, id: BlockId, span: Span) {
        self.block_mut(id).extend_span(span);
    }

    fn push_node(&mut self, target: BlockId, node: BlockNode) {
        let span = node.span();
        self.extend(target, span);
        self.block_mut(target).nodes.push(node);
    }

    fn block_mut(&mut self, id: BlockId) -> &mut Block {
        let idx = id.index();
        &mut self.0[idx]
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Block {
    span: Span,
    nodes: Vec<BlockNode>,
}

impl Block {
    fn new(span: Span) -> Self {
        Self {
            span,
            nodes: Vec::new(),
        }
    }

    fn extend_span(&mut self, span: Span) {
        if self.nodes.is_empty() && self.span.length() == 0 {
            self.span = span;
            return;
        }

        let start = self.span.start().min(span.start());
        let end = self.span.end().max(span.end());
        self.span = Span::from_bounds(start as usize, end as usize);
    }
}

#[derive(Clone, Debug, Serialize)]
pub enum BlockNode {
    Leaf {
        label: String,
        span: Span,
    },
    Block {
        name: String,
        span: Span,
        body: BlockId,
    },
    Segment {
        label: String,
        span: Span,
        body: BlockId,
    },
    Error {
        message: String,
        span: Span,
    },
}

impl BlockNode {
    fn span(&self) -> Span {
        match self {
            BlockNode::Leaf { span, .. }
            | BlockNode::Block { span, .. }
            | BlockNode::Segment { span, .. }
            | BlockNode::Error { span, .. } => *span,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{templatetags::django_builtin_specs, TagSpecs};
    use camino::Utf8Path;
    use djls_source::File;
    use djls_templates::parse_template;
    use djls_workspace::FileSystem;
    use djls_workspace::InMemoryFileSystem;
    use std::sync::Arc;
    use std::sync::Mutex;

    #[salsa::db]
    #[derive(Clone)]
    struct TestDatabase {
        storage: salsa::Storage<Self>,
        fs: Arc<Mutex<InMemoryFileSystem>>,
    }

    impl TestDatabase {
        fn new() -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
            }
        }

        fn add_file(&self, path: &str, content: &str) {
            self.fs
                .lock()
                .unwrap()
                .add_file(path.into(), content.to_string());
        }
    }

    #[salsa::db]
    impl salsa::Database for TestDatabase {}

    #[salsa::db]
    impl djls_source::Db for TestDatabase {
        fn read_file_source(&self, path: &Utf8Path) -> std::io::Result<String> {
            self.fs.lock().unwrap().read_to_string(path)
        }
    }

    #[salsa::db]
    impl djls_templates::Db for TestDatabase {}

    #[salsa::db]
    impl crate::db::Db for TestDatabase {
        fn tag_specs(&self) -> Arc<TagSpecs> {
            Arc::new(django_builtin_specs())
        }
    }

    #[test]
    fn test_block_tree_building() {
        let db = TestDatabase::new();

        let source = r"
{% block header %}
    <h1>Title</h1>
{% endblock header %}

{% if user.is_authenticated %}
    <p>Welcome {{ user.name }}</p>
    {% if user.is_staff %}
        <span>Admin</span>
    {% else %}
        <span>Regular user</span>
    {% endif %}
{% else %}
    <p>Please log in</p>
{% endif %}

{% for item in items %}
    <li>{{ item }}</li>
{% endfor %}
";

        db.add_file("test.html", source);
        let file = File::new(&db, "test.html".into(), 0);
        let nodelist = parse_template(&db, file).expect("should parse");
        let tag_shapes = TagShapes::from_specs(&db.tag_specs());
        let block_tree = BlockTree::build(&db, nodelist, &tag_shapes);
        insta::assert_yaml_snapshot!(block_tree);
    }
}

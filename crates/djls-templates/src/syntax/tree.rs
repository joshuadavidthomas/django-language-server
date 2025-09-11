use crate::ast::LineOffsets;
use crate::ast::Span;
use crate::db::Db as TemplateDb;
use crate::syntax::meta::TagMeta;

#[salsa::interned(debug)]
pub struct SyntaxNodeId<'db> {
    pub node: SyntaxNode<'db>,
}

impl<'db> SyntaxNodeId<'db> {
    /// Resolve this ID to the actual node
    pub fn resolve(&self, db: &'db dyn TemplateDb) -> SyntaxNode<'db> {
        self.node(db)
    }

    /// Check if this node is a specific tag type
    pub fn is_tag(&self, db: &'db dyn TemplateDb, tag_name: &str) -> bool {
        match &self.resolve(db) {
            SyntaxNode::Tag(tag_node) => tag_node.name.text(db) == tag_name,
            _ => false,
        }
    }

    /// Get the span of this node
    pub fn span(&self, db: &'db dyn TemplateDb) -> Option<Span> {
        match &self.resolve(db) {
            SyntaxNode::Tag(tag_node) => Some(tag_node.span),
            SyntaxNode::Text(text_node) => Some(text_node.span),
            SyntaxNode::Variable(var_node) => Some(var_node.span),
            SyntaxNode::Comment(comment_node) => Some(comment_node.span),
            SyntaxNode::Error { span, .. } => Some(*span),
            SyntaxNode::Root { .. } => None,
        }
    }

    /// Get all children of this node (for hierarchical nodes)
    pub fn children(&self, db: &'db dyn TemplateDb) -> Vec<SyntaxNodeId<'db>> {
        match &self.resolve(db) {
            SyntaxNode::Root { children } => children.clone(),
            SyntaxNode::Tag(tag_node) => tag_node.children.clone(),
            _ => Vec::new(),
        }
    }

    /// Check if this node has any children
    pub fn has_children(&self, db: &'db dyn TemplateDb) -> bool {
        !self.children(db).is_empty()
    }

    /// Get the first child of this node
    pub fn first_child(&self, db: &'db dyn TemplateDb) -> Option<SyntaxNodeId<'db>> {
        self.children(db).into_iter().next()
    }

    /// Get the last child of this node
    pub fn last_child(&self, db: &'db dyn TemplateDb) -> Option<SyntaxNodeId<'db>> {
        self.children(db).into_iter().last()
    }

    /// Find all descendant nodes that match a predicate
    pub fn find_descendants<F>(
        &self,
        db: &'db dyn TemplateDb,
        predicate: F,
    ) -> Vec<SyntaxNodeId<'db>>
    where
        F: Fn(&SyntaxNodeId<'db>) -> bool + Copy,
    {
        let mut result = Vec::new();
        self.collect_descendants_recursive(db, predicate, &mut result);
        result
    }

    fn collect_descendants_recursive<F>(
        self,
        db: &'db dyn TemplateDb,
        predicate: F,
        result: &mut Vec<SyntaxNodeId<'db>>,
    ) where
        F: Fn(&SyntaxNodeId<'db>) -> bool + Copy,
    {
        for child in self.children(db) {
            if predicate(&child) {
                result.push(child);
            }
            child.collect_descendants_recursive(db, predicate, result);
        }
    }

    /// Find all descendant tag nodes with a specific name
    pub fn find_tags_by_name(
        &self,
        db: &'db dyn TemplateDb,
        tag_name: &str,
    ) -> Vec<SyntaxNodeId<'db>> {
        self.find_descendants(db, |node| node.is_tag(db, tag_name))
    }

    /// Find the nearest ancestor that matches a predicate
    pub fn find_ancestor<F>(
        &self,
        db: &'db dyn TemplateDb,
        tree: &SyntaxTree<'db>,
        predicate: F,
    ) -> Option<SyntaxNodeId<'db>>
    where
        F: Fn(&SyntaxNodeId<'db>) -> bool,
    {
        self.find_parent(db, tree).and_then(|parent| {
            if predicate(&parent) {
                Some(parent)
            } else {
                parent.find_ancestor(db, tree, predicate)
            }
        })
    }

    /// Find the parent of this node in the tree
    pub fn find_parent(
        &self,
        db: &'db dyn TemplateDb,
        tree: &SyntaxTree<'db>,
    ) -> Option<SyntaxNodeId<'db>> {
        Self::find_parent_recursive(db, tree.root(db), *self)
    }

    fn find_parent_recursive(
        db: &'db dyn TemplateDb,
        parent: SyntaxNodeId<'db>,
        target: SyntaxNodeId<'db>,
    ) -> Option<SyntaxNodeId<'db>> {
        for child in parent.children(db) {
            if child == target {
                return Some(parent);
            }
            if let Some(found) = Self::find_parent_recursive(db, child, target) {
                return Some(found);
            }
        }
        None
    }

    /// Get all sibling nodes (nodes with the same parent)
    pub fn siblings(
        &self,
        db: &'db dyn TemplateDb,
        tree: &SyntaxTree<'db>,
    ) -> Vec<SyntaxNodeId<'db>> {
        if let Some(parent) = self.find_parent(db, tree) {
            parent.children(db)
        } else {
            // Root level siblings
            tree.children(db)
        }
    }

    /// Get the next sibling node
    pub fn next_sibling(
        &self,
        db: &'db dyn TemplateDb,
        tree: &SyntaxTree<'db>,
    ) -> Option<SyntaxNodeId<'db>> {
        let siblings = self.siblings(db, tree);
        let mut found_self = false;
        for sibling in siblings {
            if found_self {
                return Some(sibling);
            }
            if sibling == *self {
                found_self = true;
            }
        }
        None
    }

    /// Get the previous sibling node
    pub fn prev_sibling(
        &self,
        db: &'db dyn TemplateDb,
        tree: &SyntaxTree<'db>,
    ) -> Option<SyntaxNodeId<'db>> {
        let siblings = self.siblings(db, tree);
        let mut prev = None;
        for sibling in siblings {
            if sibling == *self {
                return prev;
            }
            prev = Some(sibling);
        }
        None
    }

    /// Get depth-first traversal of this node and all its descendants
    pub fn depth_first_traversal(&self, db: &'db dyn TemplateDb) -> Vec<SyntaxNodeId<'db>> {
        let mut result = vec![*self];
        for child in self.children(db) {
            result.extend(child.depth_first_traversal(db));
        }
        result
    }

    /// Get breadth-first traversal of this node and all its descendants
    pub fn breadth_first_traversal(&self, db: &'db dyn TemplateDb) -> Vec<SyntaxNodeId<'db>> {
        let mut result = Vec::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(*self);

        while let Some(node) = queue.pop_front() {
            result.push(node);
            for child in node.children(db) {
                queue.push_back(child);
            }
        }

        result
    }

    /// Check if this node is an ancestor of another node
    pub fn is_ancestor_of(&self, db: &'db dyn TemplateDb, other: &SyntaxNodeId<'db>) -> bool {
        for descendant in self.depth_first_traversal(db) {
            if descendant == *other {
                return true;
            }
        }
        false
    }

    /// Get the depth of this node in the tree (root is depth 0)
    pub fn depth(&self, db: &'db dyn TemplateDb, tree: &SyntaxTree<'db>) -> usize {
        let mut depth = 0;
        let mut current = *self;

        while let Some(parent) = current.find_parent(db, tree) {
            depth += 1;
            current = parent;
        }

        depth
    }

    /// Find all nodes within a specific scope (useful for variable analysis)
    pub fn scope_nodes(&self, db: &'db dyn TemplateDb) -> Vec<SyntaxNodeId<'db>> {
        match &self.resolve(db) {
            SyntaxNode::Tag(tag_node) if tag_node.meta.can_have_children() => {
                // This is a block tag - return all its descendants
                self.depth_first_traversal(db)
            }
            _ => {
                // Non-block node - only itself
                vec![*self]
            }
        }
    }
}

#[salsa::tracked]
pub struct SyntaxTree<'db> {
    #[tracked]
    pub root: SyntaxNodeId<'db>,
    #[tracked]
    #[returns(ref)]
    pub line_offsets: LineOffsets,
}

impl<'db> SyntaxTree<'db> {
    /// Create a new empty syntax tree
    pub fn empty(db: &'db dyn crate::db::Db) -> Self {
        let root = SyntaxNode::Root {
            children: Vec::new(),
        };
        let root_id = SyntaxNodeId::new(db, root);
        let line_offsets = LineOffsets::default();

        SyntaxTree::new(db, root_id, line_offsets)
    }

    /// Get all child nodes of the root
    pub fn children(&self, db: &'db dyn crate::db::Db) -> Vec<SyntaxNodeId<'db>> {
        match &self.root(db).resolve(db) {
            SyntaxNode::Root { children } => children.clone(),
            _ => Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum SyntaxNode<'db> {
    Root { children: Vec<SyntaxNodeId<'db>> },
    Tag(TagNode<'db>),
    Text(TextNode),
    Variable(VariableNode<'db>),
    Comment(CommentNode),
    Error { message: String, span: Span },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct TagNode<'db> {
    pub name: TagName<'db>,
    pub bits: Vec<String>,
    pub span: Span,
    pub meta: TagMeta<'db>,
    pub children: Vec<SyntaxNodeId<'db>>,
}

#[salsa::interned(debug)]
pub struct TagName<'db> {
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct TextNode {
    pub content: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct VariableNode<'db> {
    pub var: VariableName<'db>,
    pub filters: Vec<FilterName<'db>>,
    pub span: Span,
}

#[salsa::interned(debug)]
pub struct VariableName<'db> {
    pub text: String,
}

#[salsa::interned(debug)]
pub struct FilterName<'db> {
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct CommentNode {
    pub content: String,
    pub span: Span,
}

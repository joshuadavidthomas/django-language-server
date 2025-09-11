//! Fragment system for storing Django template tag bodies and branch contents.
//!
//! Fragments are mini-nodelists that belong to block tags, following Django's pattern.
//! This module provides efficient storage and management of tag body contents with
//! Salsa integration for incremental computation.
//!
//! ## Key Components
//!
//! - [`Fragment`]: Individual fragment with owner tracking
//!
//! ## Fragment Patterns
//!
//! - **Singleton tags** (e.g., `{% include %}`) → no fragments
//! - **Simple blocks** (e.g., `{% block %}...{% endblock %}`) → 1 fragment
//! - **Branched blocks** (e.g., `{% if %}...{% elif %}...{% else %}...{% endif %}`) → 1 fragment per branch (max 3)
//!
//! ## Example Usage
//!
//! ```ignore
//! // Create fragment for a block tag
//! let fragment = Fragment::new(db, tag_node_id, vec![], None);
//!
//! // Create fragments for if/elif/else branches
//! let if_fragment = Fragment::new(db, if_tag_id, vec![child1, child2], Some("if".to_string()));
//! ```

use crate::syntax_tree::SyntaxNodeId;

/// Individual fragment containing tag body contents
#[salsa::tracked]
pub struct Fragment<'db> {
    #[tracked]
    pub owner: SyntaxNodeId<'db>,       // The TagNode that owns this fragment
    #[tracked]
    #[returns(ref)]
    pub children: Vec<SyntaxNodeId<'db>>, // Body contents
    #[tracked]
    #[returns(ref)]
    pub branch_kind: Option<String>,     // For if/elif/else branches
}

impl<'db> Fragment<'db> {
    /// Create a new fragment with the given parameters
    pub fn create(
        db: &'db dyn crate::db::Db,
        owner: SyntaxNodeId<'db>,
        children: Vec<SyntaxNodeId<'db>>,
        branch_kind: Option<String>,
    ) -> Self {
        Fragment::new(db, owner, children, branch_kind)
    }

    /// Add a child to this fragment, returning a new fragment
    #[must_use]
    pub fn add_child(self, db: &'db dyn crate::db::Db, child: SyntaxNodeId<'db>) -> Self {
        let mut new_children = self.children(db).clone();
        new_children.push(child);
        Fragment::new(db, self.owner(db), new_children, self.branch_kind(db).clone())
    }

    /// Check if this fragment is for a specific branch kind
    pub fn is_branch(self, db: &'db dyn crate::db::Db, branch_kind: &str) -> bool {
        self.branch_kind(db)
            .as_ref()
            .is_some_and(|kind| kind == branch_kind)
    }

    /// Check if this fragment has any children
    pub fn has_children(self, db: &'db dyn crate::db::Db) -> bool {
        !self.children(db).is_empty()
    }

    /// Get the number of children in this fragment
    pub fn child_count(self, db: &'db dyn crate::db::Db) -> usize {
        self.children(db).len()
    }

    /// Check if this fragment contains a specific child node
    pub fn contains_child(self, db: &'db dyn crate::db::Db, child: SyntaxNodeId<'db>) -> bool {
        self.children(db).contains(&child)
    }
}

/// Fragment store using vectors and helper functions for managing fragments
#[salsa::tracked]
pub struct FragmentStore<'db> {
    #[tracked]
    #[returns(ref)]
    fragments: Vec<Fragment<'db>>,
}

impl<'db> FragmentStore<'db> {
    /// Create a new empty fragment store
    pub fn empty(db: &'db dyn crate::db::Db) -> Self {
        FragmentStore::new(db, Vec::new())
    }

    /// Add a fragment to this store, returning a new store
    #[must_use]
    pub fn add_fragment(self, db: &'db dyn crate::db::Db, fragment: Fragment<'db>) -> Self {
        let mut fragments = self.fragments(db).clone();
        fragments.push(fragment);
        FragmentStore::new(db, fragments)
    }

    /// Get all fragments in the store
    pub fn all_fragments(self, db: &'db dyn crate::db::Db) -> &'db [Fragment<'db>] {
        self.fragments(db)
    }

    /// Get fragment count
    pub fn count(self, db: &'db dyn crate::db::Db) -> usize {
        self.fragments(db).len()
    }

    /// Get fragments owned by a specific node
    pub fn fragments_for_owner(self, db: &'db dyn crate::db::Db, owner: SyntaxNodeId<'db>) -> Vec<Fragment<'db>> {
        self.fragments(db)
            .iter()
            .filter(|fragment| fragment.owner(db) == owner)
            .copied()
            .collect()
    }

    /// Get fragments by branch kind for a specific owner
    pub fn fragments_by_branch(
        self,
        db: &'db dyn crate::db::Db,
        owner: SyntaxNodeId<'db>,
        branch_kind: &str,
    ) -> Vec<Fragment<'db>> {
        self.fragments(db)
            .iter()
            .filter(|fragment| {
                fragment.owner(db) == owner && fragment.is_branch(db, branch_kind)
            })
            .copied()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax_tree::{SyntaxNode, TextNode};
    use crate::ast::Span;

    #[salsa::db]
    #[derive(Clone)]
    struct TestDatabase {
        storage: salsa::Storage<Self>,
    }

    impl TestDatabase {
        fn new() -> Self {
            Self {
                storage: salsa::Storage::default(),
            }
        }
    }

    #[salsa::db]
    impl salsa::Database for TestDatabase {}

    #[salsa::db]
    impl djls_workspace::Db for TestDatabase {
        fn fs(&self) -> std::sync::Arc<dyn djls_workspace::FileSystem> {
            use djls_workspace::InMemoryFileSystem;
            static FS: std::sync::OnceLock<std::sync::Arc<InMemoryFileSystem>> =
                std::sync::OnceLock::new();
            FS.get_or_init(|| std::sync::Arc::new(InMemoryFileSystem::default()))
                .clone()
        }

        fn read_file_content(&self, path: &std::path::Path) -> Result<String, std::io::Error> {
            std::fs::read_to_string(path)
        }
    }

    #[salsa::db]
    impl crate::db::Db for TestDatabase {
        fn tag_specs(&self) -> std::sync::Arc<crate::templatetags::TagSpecs> {
            std::sync::Arc::new(crate::templatetags::TagSpecs::default())
        }
    }

    // Helper tracked function to create test data
    #[salsa::tracked]
    fn create_test_fragment<'db>(
        db: &'db dyn crate::db::Db,
        content: &'db str,
        branch_kind: Option<String>,
    ) -> Fragment<'db> {
        let node = SyntaxNode::Text(TextNode {
            content: content.to_string(),
            span: Span::new(0, u32::try_from(content.len()).unwrap_or(0)),
        });
        let owner = SyntaxNodeId::new(db, node);
        Fragment::create(db, owner, vec![], branch_kind)
    }

    #[test]
    fn test_fragment_creation() {
        let db = TestDatabase::new();
        
        let fragment = create_test_fragment(&db, "test", None);
        assert!(!fragment.has_children(&db));
        assert_eq!(fragment.child_count(&db), 0);
    }

    #[test]
    fn test_fragment_with_branch() {
        let db = TestDatabase::new();
        
        let fragment = create_test_fragment(&db, "if_tag", Some("if".to_string()));
        assert!(fragment.is_branch(&db, "if"));
        assert!(!fragment.is_branch(&db, "elif"));
        assert_eq!(fragment.branch_kind(&db), &Some("if".to_string()));
    }

    #[salsa::tracked]
    fn test_store_operations(db: &dyn crate::db::Db) -> (usize, usize) {
        let store = FragmentStore::empty(db);
        let initial_count = store.count(db);
        
        let fragment = create_test_fragment(db, "test", None);
        let store = store.add_fragment(db, fragment);
        let final_count = store.count(db);
        
        (initial_count, final_count)
    }

    #[salsa::tracked]
    fn test_store_filtering_operations(db: &dyn crate::db::Db) -> (usize, usize, usize) {
        let fragment1 = create_test_fragment(db, "if_tag", Some("if".to_string()));
        let fragment2 = create_test_fragment(db, "if_tag", Some("elif".to_string()));
        let fragment3 = create_test_fragment(db, "other_tag", None);
        
        let store = FragmentStore::empty(db)
            .add_fragment(db, fragment1)
            .add_fragment(db, fragment2)
            .add_fragment(db, fragment3);
        
        let total_count = store.count(db);
        
        // Test filtering by owner
        let if_fragments = store.fragments_for_owner(db, fragment1.owner(db));
        let owner_count = if_fragments.len();
        
        // Test filtering by branch
        let elif_fragments = store.fragments_by_branch(db, fragment1.owner(db), "elif");
        let branch_count = elif_fragments.len();
        
        (total_count, owner_count, branch_count)
    }

    #[test]
    fn test_fragment_store() {
        let db = TestDatabase::new();
        let (initial_count, final_count) = test_store_operations(&db);
        
        assert_eq!(initial_count, 0);
        assert_eq!(final_count, 1);
    }

    #[test] 
    fn test_fragment_store_filtering() {
        let db = TestDatabase::new();
        let (total_count, owner_count, branch_count) = test_store_filtering_operations(&db);
        
        assert_eq!(total_count, 3);
        assert_eq!(owner_count, 2);
        assert_eq!(branch_count, 1);
    }
}
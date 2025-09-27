//! Template inheritance resolution with cycle recovery
//! Handles Django's extends/blocks mechanism with inspector integration

use camino::Utf8PathBuf;
use djls_source::{File, Span};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::interned::{TagName, TemplatePath};
use crate::semantic_types::BlockNode;

/// A resolved block in the inheritance chain
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedBlock<'db> {
    pub name: TagName<'db>,
    pub content: Vec<Span>, // Just use spans for now
    pub source_template: TemplatePath<'db>,
}

/// Map of block names to their resolved content
pub type BlockMap<'db> = Vec<(TagName<'db>, Vec<Span>, TemplatePath<'db>)>;

/// Template with resolved inheritance chain
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedTemplate<'db> {
    /// This template's path
    pub path: TemplatePath<'db>,

    /// Parent template path if this extends another
    pub parent: Option<TemplatePath<'db>>,

    /// Blocks defined in this template
    pub blocks: BlockMap<'db>,

    /// Source file reference
    pub file: File,

    /// Position info
    pub source_span: Span,
}

impl<'db> ResolvedTemplate<'db> {
    /// Resolve a block by name, traversing the inheritance chain
    pub fn resolve_block(&self, db: &'db dyn crate::Db, name: TagName<'db>) -> ResolvedBlock<'db> {
        // Check local blocks first
        for (block_name, content, source_template) in &self.blocks {
            if *block_name == name {
                return ResolvedBlock {
                    name,
                    content: content.clone(),
                    source_template: *source_template,
                };
            }
        }

        // Check parent chain
        if let Some(parent_path) = self.parent {
            let parent = resolve_template(db, parent_path);
            return parent.resolve_block(db, name);
        }

        // Not found - return empty block
        ResolvedBlock {
            name,
            content: vec![],
            source_template: self.path,
        }
    }
}

/// Context for template resolution
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateContext<'db> {
    pub variables: FxHashMap<String, crate::semantic_types::Type>,
    pub blocks: Vec<TagName<'db>>,
}

/// Resolve a template path to a fully resolved template with inheritance
/// Uses djls-project inspector for path resolution
pub fn resolve_template<'db>(
    db: &'db dyn crate::Db,
    path: TemplatePath<'db>,
) -> ResolvedTemplate<'db> {
    // TODO: Use djls-project inspector to resolve actual file path
    // For now, create a placeholder implementation

    // This would normally:
    // 1. Use inspector to resolve template path to file system path
    // 2. Load and parse the template file
    // 3. Extract extends directive if present
    // 4. Recursively resolve parent template
    // 5. Extract and merge blocks

    // Placeholder: create empty resolved template
    let file = File::new(db, path.path(db).clone(), 0);
    let blocks = BlockMap::default();

    ResolvedTemplate {
        path,
        parent: None, // No parent in placeholder
        blocks,
        file,
        source_span: Span::new(0, 0),
    }
}

/// Builder for inheritance resolution with complex logic
pub struct InheritanceResolver<'db> {
    db: &'db dyn crate::Db,
    visited: FxHashSet<TemplatePath<'db>>,
    block_stack: Vec<TagName<'db>>,
}

impl<'db> InheritanceResolver<'db> {
    /// Create a new inheritance resolver
    pub fn new(db: &'db dyn crate::Db) -> Self {
        Self {
            db,
            visited: FxHashSet::default(),
            block_stack: Vec::new(),
        }
    }

    /// Resolve template with cycle detection
    pub fn resolve(&mut self, path: TemplatePath<'db>) -> Result<ResolvedTemplate, String> {
        // Check for cycles
        if !self.visited.insert(path) {
            return Err(format!("Circular inheritance detected at {:?}", path));
        }

        // Use the tracked query for actual resolution
        let resolved = resolve_template(self.db, path);

        self.visited.remove(&path);
        Ok(resolved)
    }

    /// Build the final resolved template
    pub fn build(self) -> ResolvedTemplate<'db> {
        // This would normally build the final template
        // For now, just return a placeholder
        let path = TemplatePath::new(self.db, Utf8PathBuf::new());
        resolve_template(self.db, path)
    }

    /// Merge blocks from parent and child templates
    fn merge_blocks<'a>(
        &mut self,
        parent_blocks: &'a BlockMap<'db>,
        child_blocks: &'a BlockMap<'db>,
    ) -> BlockMap<'db> {
        let mut merged = parent_blocks.clone();

        // Override parent blocks with child blocks
        for entry in child_blocks.iter() {
            let (name, content, template) = entry;
            // Find and replace existing block, or append if not found
            if let Some(pos) = merged.iter().position(|(n, _, _)| n == name) {
                merged[pos] = (*name, content.clone(), *template);
            } else {
                merged.push((*name, content.clone(), *template));
            }
        }

        merged
    }

    /// Handle super blocks (blocks that extend parent blocks)
    fn handle_super_block(
        &mut self,
        block: &BlockNode<'db>,
        parent_block: Option<&[Span]>,
    ) -> Vec<Span> {
        let mut content = Vec::new();

        // Add parent content if super is used
        if let Some(parent) = parent_block {
            content.extend_from_slice(parent);
        }

        // Add child content
        content.extend(block.content.clone());

        content
    }
}

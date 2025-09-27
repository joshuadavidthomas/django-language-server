use djls_source::Span;

use crate::blocks::BlockId;
use crate::blocks::BlockTree;
use crate::db::Db;
use crate::ids::BlockDefinition;
use crate::ids::SemanticElement;
use crate::ids::TagReference;
use crate::ids::TemplateDependency;
use crate::ids::VariableInfo;
use crate::ids::VariableReference;
use crate::queries::build_block_tree;
use crate::queries::build_semantic_forest;
use crate::queries::collect_block_definitions;
use crate::queries::collect_template_dependencies;
use crate::queries::collect_variables;
use crate::queries::compute_tag_spans;
use crate::queries::find_containing_tag;
use crate::queries::find_element_at_offset;
use crate::queries::find_enclosing_block;
use crate::queries::find_variable_at_offset;
use crate::queries::validate_template;
use crate::semantic::forest::SemanticForest;
use crate::ValidationError;

/// Facade for all semantic analysis of a Django template.
/// This is the primary entry point for LSP operations.
pub struct SemanticIndex<'db> {
    db: &'db dyn Db,
    nodelist: djls_templates::NodeList<'db>,
}

impl<'db> SemanticIndex<'db> {
    /// Create a new semantic index for a parsed template
    pub fn new(db: &'db dyn Db, nodelist: djls_templates::NodeList<'db>) -> Self {
        Self { db, nodelist }
    }
    
    // ===== Core Components =====
    
    /// Get the block tree (structural representation)
    pub fn block_tree(&self) -> BlockTree<'db> {
        build_block_tree(self.db, self.nodelist)
    }
    
    /// Get the semantic forest (enriched with arguments)
    pub fn semantic_forest(&self) -> SemanticForest<'db> {
        build_semantic_forest(self.db, self.nodelist)
    }
    
    /// Get all tag spans for syntax highlighting
    pub fn tag_spans(&self) -> Vec<Span> {
        compute_tag_spans(self.db, self.nodelist)
    }
    
    // ===== Query Operations =====
    
    /// Find the semantic element at the given offset
    pub fn find_at_offset(&self, offset: u32) -> SemanticElement {
        find_element_at_offset(self.db, self.nodelist, offset)
    }
    
    /// Get the enclosing block at the given offset
    pub fn enclosing_block(&self, offset: u32) -> Option<BlockId> {
        find_enclosing_block(self.db, self.nodelist, offset)
    }
    
    /// Find the tag or segment containing this offset
    pub fn containing_tag(&self, offset: u32) -> Option<TagReference> {
        find_containing_tag(self.db, self.nodelist, offset)
    }
    
    // ===== Validation =====
    
    /// Trigger validation (errors are accumulated via salsa)
    pub fn validate(&self) {
        validate_template(self.db, self.nodelist);
    }
    
    /// Get validation errors that have been accumulated
    pub fn validation_errors(&self) -> Vec<ValidationError> {
        // Trigger validation first
        self.validate();
        // Then get accumulated errors
        validate_template::accumulated::<crate::db::ValidationErrorAccumulator>(self.db, self.nodelist)
            .iter()
            .map(|acc| acc.0.clone())
            .collect()
    }
    
    /// Check if the template is valid
    pub fn is_valid(&self) -> bool {
        self.validation_errors().is_empty()
    }
    
    // ===== Variables & Scopes =====
    
    /// Get all variable references in the template
    pub fn variables(&self) -> Vec<VariableReference> {
        collect_variables(self.db, self.nodelist)
    }
    
    /// Find variable definition/usage at offset
    pub fn variable_at_offset(&self, offset: u32) -> Option<VariableInfo> {
        find_variable_at_offset(self.db, self.nodelist, offset)
    }
    
    // ===== Template Inheritance =====
    
    /// Get blocks defined in this template
    pub fn defined_blocks(&self) -> Vec<BlockDefinition> {
        collect_block_definitions(self.db, self.nodelist)
    }
    
    /// Get extended/included templates
    pub fn dependencies(&self) -> Vec<TemplateDependency> {
        collect_template_dependencies(self.db, self.nodelist)
    }
}
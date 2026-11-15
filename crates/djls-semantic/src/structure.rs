//! Semantic structure for Django templates.
//!
//! `TemplateTree` is a structural semantic projection over
//! `djls_templates::NodeList`. It preserves parsed tag bits, block hierarchy,
//! standalone tags, variables, and non-tag regions needed by semantic features.
//!
//! It is not intended to be a lossless syntax tree. Parser-owned details that do
//! not affect structure, such as exact parse errors, remain available from the
//! original `NodeList`.

pub(crate) mod active;
pub(crate) mod builder;
pub(crate) mod grammar;
pub use grammar::GrammarOpeningDefinition;
pub use grammar::SemanticGrammarVocabulary;
pub use grammar::semantic_grammar_vocabulary;
mod source;
pub(crate) mod tree;

use djls_templates::NodeList;
use salsa::Accumulator;

use crate::ValidationErrorAccumulator;
use crate::db::Db;
pub(crate) use crate::structure::active::ActiveTemplateNode;
pub(crate) use crate::structure::active::ActiveTemplateTag;
pub(crate) use crate::structure::active::ActiveTemplateVariable;
pub(crate) use crate::structure::active::CapturedClosingTag;
pub(crate) use crate::structure::active::StructuralOccurrenceMeaning;
pub(crate) use crate::structure::active::active_template_nodes;
pub(crate) use crate::structure::active::active_template_tags;
pub(crate) use crate::structure::builder::TemplateTreeBuilder;
pub(crate) use crate::structure::grammar::TagClassification;
pub use crate::structure::source::SourceBits;
pub use crate::structure::source::SourceFilters;
pub use crate::structure::source::SourceNode;
pub use crate::structure::source::SourceRegion;
pub use crate::structure::source::SourceStructure;
pub use crate::structure::source::template_structure;
pub use crate::structure::source::template_structure_with_roles;
pub use crate::structure::tree::BlockRole;
pub use crate::structure::tree::RegionId;
pub(crate) use crate::structure::tree::Regions;
pub use crate::structure::tree::TemplateNode;
pub use crate::structure::tree::TemplateRegion;
pub use crate::structure::tree::TemplateTree;

/// Builds the tree in source positions from the projectless tag grammar.
#[salsa::tracked(returns(copy))]
pub fn build_projectless_template_tree<'db>(
    db: &'db dyn Db,
    nodelist: NodeList<'db>,
) -> TemplateTree<'db> {
    let grammar = grammar::SparseTagGrammar::projectless(db, nodelist);
    let tree_data = TemplateTreeBuilder::new(&grammar).model_data(db, nodelist);
    for error in &tree_data.diagnostics {
        ValidationErrorAccumulator(error.clone()).accumulate(db);
    }
    tree_data.into_tree(db)
}

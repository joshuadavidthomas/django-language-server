//! Semantic structure for Django templates.
//!
//! `TemplateTree` is a structural semantic projection over
//! `djls_templates::NodeList`. It preserves source spans, parsed tag bits,
//! block hierarchy, standalone tags, variables, and non-tag regions needed by
//! semantic features such as opaque-region detection and document outlines.
//!
//! It is not intended to be a lossless syntax tree. Parser-owned details that do
//! not affect structure, such as exact parse errors, remain available from the
//! original `NodeList`.

pub(crate) mod active;
pub(crate) mod builder;
pub(crate) mod folding;
pub(crate) mod grammar;
pub use grammar::GrammarOpeningDefinition;
pub use grammar::SemanticGrammarVocabulary;
pub use grammar::semantic_grammar_vocabulary;
pub(crate) mod opaque;
pub(crate) mod outline;
pub(crate) mod tree;

use djls_source::File;
use djls_templates::NodeList;
use salsa::Accumulator;

use crate::ValidationErrorAccumulator;
use crate::db::Db;
use crate::scoping::template_analysis_projection_for_file;
pub(crate) use crate::structure::active::ActiveTemplateNode;
pub(crate) use crate::structure::active::ActiveTemplateTag;
pub(crate) use crate::structure::active::ActiveTemplateVariable;
pub(crate) use crate::structure::active::CapturedClosingTag;
pub(crate) use crate::structure::active::StructuralOccurrenceMeaning;
pub(crate) use crate::structure::active::active_template_nodes;
pub(crate) use crate::structure::active::active_template_tags;
pub(crate) use crate::structure::builder::TemplateTreeBuilder;
pub use crate::structure::folding::TemplateFold;
pub use crate::structure::folding::TemplateFoldKind;
pub use crate::structure::folding::build_template_folds_for_file;
pub(crate) use crate::structure::grammar::TagClassification;
pub use crate::structure::opaque::OpaqueRegions;
pub use crate::structure::opaque::compute_opaque_regions;
pub use crate::structure::outline::OutlineItem;
pub use crate::structure::outline::OutlineKind;
pub use crate::structure::outline::build_template_outline_for_file;
pub use crate::structure::tree::BlockRole;
pub use crate::structure::tree::RegionId;
pub(crate) use crate::structure::tree::Regions;
pub use crate::structure::tree::TemplateNode;
pub use crate::structure::tree::TemplateRegion;
pub use crate::structure::tree::TemplateTree;

#[salsa::tracked(returns(copy))]
pub fn build_template_tree_for_file<'db>(
    db: &'db dyn Db,
    file: File,
    nodelist: NodeList<'db>,
) -> TemplateTree<'db> {
    if db.project().is_none() {
        return build_projectless_template_tree(db, nodelist);
    }
    // Production readers walk the analysis tree in layout coordinates. This copy in source
    // coordinates exists for callers that want the whole tree.
    let projection = template_analysis_projection_for_file(db, file, nodelist);
    let positions = projection.positions;
    for error in projection.analysis.diagnostics(db) {
        ValidationErrorAccumulator(positions.error(error.clone())).accumulate(db);
    }
    let tree = projection.analysis.tree(db);
    let mut regions = tree.regions(db).clone();
    regions.map_spans(|span| positions.span(span));
    TemplateTree::new(db, tree.root(db), regions)
}

fn build_projectless_template_tree<'db>(
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

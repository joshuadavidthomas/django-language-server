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

pub(crate) mod builder;
pub(crate) mod grammar;
pub(crate) mod opaque;
pub(crate) mod outline;
#[cfg(test)]
pub(crate) mod snapshot;
pub(crate) mod tree;

pub(crate) use builder::TemplateTreeBuilder;
pub(crate) use grammar::TagIndex;
pub use opaque::compute_opaque_regions;
pub(crate) use opaque::OpaqueRegions;
pub use outline::build_template_outline;
pub use outline::OutlineItem;
pub use outline::OutlineKind;
pub(crate) use tree::BlockRole;
pub(crate) use tree::RegionId;
pub(crate) use tree::Regions;
pub(crate) use tree::TemplateNode;
pub(crate) use tree::TemplateTree;

use crate::db::Db;
use crate::traits::SemanticModel;

#[salsa::tracked]
pub fn build_template_tree<'db>(
    db: &'db dyn Db,
    nodelist: djls_templates::NodeList<'db>,
) -> TemplateTree<'db> {
    let builder = TemplateTreeBuilder::new(db, TagIndex::from_specs(db));
    builder.model(db, nodelist)
}

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
pub(crate) mod tree;

use crate::db::Db;
pub(crate) use crate::structure::builder::TemplateTreeBuilder;
pub(crate) use crate::structure::grammar::compute_tag_index;
pub use crate::structure::opaque::OpaqueRegions;
pub use crate::structure::opaque::compute_opaque_regions;
pub use crate::structure::outline::OutlineItem;
pub use crate::structure::outline::OutlineKind;
pub use crate::structure::outline::build_template_outline;
pub use crate::structure::tree::BlockRole;
pub use crate::structure::tree::RegionId;
pub use crate::structure::tree::Regions;
pub use crate::structure::tree::TemplateNode;
pub use crate::structure::tree::TemplateRegion;
pub use crate::structure::tree::TemplateTree;

#[salsa::tracked]
pub fn build_template_tree<'db>(
    db: &'db dyn Db,
    nodelist: djls_templates::NodeList<'db>,
) -> TemplateTree<'db> {
    let builder = TemplateTreeBuilder::new(db, compute_tag_index(db));
    builder.model(db, nodelist)
}

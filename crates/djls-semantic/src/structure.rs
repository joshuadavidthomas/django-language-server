//! Semantic structure for Django templates.
//!
//! `TemplateTree` is a structural semantic projection over
//! `djls_templates::NodeList`. It preserves source spans, parsed tag arguments,
//! block hierarchy, standalone tags, variables, and non-tag regions needed by
//! semantic features such as opaque-region detection and document outlines.
//!
//! It is not intended to be a lossless syntax tree. Parser-owned details that do
//! not affect structure, such as exact parse errors, remain available from the
//! original `NodeList`.

pub mod builder;
pub mod grammar;
pub mod opaque;
pub mod outline;
pub mod snapshot;
pub mod tree;

pub use builder::TemplateTreeBuilder;
pub use grammar::TagIndex;
pub use opaque::compute_opaque_regions;
pub use opaque::OpaqueRegions;
pub use outline::build_template_outline;
pub use outline::OutlineItem;
pub use outline::OutlineKind;
pub use outline::TemplateOutline;
pub use tree::BlockRole;
pub use tree::RegionId;
pub use tree::Regions;
pub use tree::TemplateNode;
pub use tree::TemplateRegion;
pub use tree::TemplateTree;

use crate::db::Db;
use crate::traits::SemanticModel;

#[salsa::tracked]
pub fn build_template_tree<'db>(
    db: &'db dyn Db,
    nodelist: djls_templates::NodeList<'db>,
) -> TemplateTree<'db> {
    let builder = TemplateTreeBuilder::new(db, db.tag_index());
    builder.model(db, nodelist)
}

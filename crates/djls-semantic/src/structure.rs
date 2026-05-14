//! Semantic structure for Django templates.
//!
//! `TemplateTree` is a structural semantic projection over
//! `djls_templates::NodeList`. It preserves source spans, tag bits, block
//! hierarchy, standalone tags, and non-tag regions needed by semantic features
//! such as opaque-region detection and document outlines.
//!
//! It is not intended to be a lossless syntax tree. Parser-owned details that
//! do not affect structure, such as variable filter payloads or exact parse
//! errors, remain available from the original `NodeList`. If a future semantic
//! pass needs both tree hierarchy and parser payloads, prefer linking tree nodes
//! back to source `NodeList` indices over copying every parser field into the
//! tree.

pub mod builder;
pub mod grammar;
pub mod opaque;
pub mod snapshot;
pub mod tree;

pub use builder::TemplateTreeBuilder;
pub use grammar::TagIndex;
pub use opaque::compute_opaque_regions;
pub use opaque::OpaqueRegions;
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

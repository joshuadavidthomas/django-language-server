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

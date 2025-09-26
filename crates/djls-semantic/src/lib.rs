mod blocks;
mod db;
mod errors;
mod templatetags;
mod traits;

pub use db::Db;
pub use db::ValidationErrorAccumulator;
pub use errors::ValidationError;
pub use templatetags::django_builtin_specs;
pub use templatetags::EndTag;
pub use templatetags::TagArg;
pub use templatetags::TagSpec;
pub use templatetags::TagSpecs;

/// Validate a Django template node list and return validation errors.
///
/// This function builds a `BlockTree` from the parsed node list and, during
/// construction, accumulates semantic validation errors for issues such as:
/// - Unclosed block tags
/// - Mismatched tag pairs
/// - Orphaned intermediate tags
/// - Invalid argument counts
/// - Unmatched block names
#[salsa::tracked]
pub fn validate_nodelist(db: &dyn Db, nodelist: djls_templates::NodeList<'_>) {
    if nodelist.nodelist(db).is_empty() {
        return;
    }

    let tag_index = blocks::TagIndex::from(&db.tag_specs());
    let _ = blocks::BlockTree::build(db, nodelist, &tag_index);
}

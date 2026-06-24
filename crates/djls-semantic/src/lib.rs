mod db;
mod errors;
mod filters;
mod offset;
mod references;
mod scoping;
mod structure;
mod tags;
mod validation;

pub use db::Db;
pub use db::ValidationErrorAccumulator;
pub use djls_project::TagArgument;
pub use djls_project::TagArgumentKind;
pub use errors::ValidationError;
pub use filters::FilterAritySpecs;
pub use filters::compute_filter_arity_specs;
pub use offset::SemanticOffsetContext;
pub use references::TemplateReferenceKind;
pub use references::references_to_template_name;
pub use scoping::AvailableSymbols;
pub use scoping::LoadKind;
pub use scoping::available_symbols_at;
pub use structure::BlockRole;
pub use structure::OpaqueRegions;
pub use structure::OutlineItem;
pub use structure::OutlineKind;
pub use structure::RegionId;
pub use structure::TagClass;
pub use structure::TagIndex;
pub use structure::TemplateNode;
pub use structure::TemplateRegion;
pub use structure::TemplateTree;
pub use structure::build_template_outline;
pub use structure::build_template_tree;
pub use structure::compute_opaque_regions;
pub use structure::compute_tag_index;
pub use tags::EndTag;
pub use tags::IntermediateTag;
pub use tags::TagRole;
pub use tags::TagSpec;
pub use tags::TagSpecs;
pub use tags::builtin_tag_specs;
pub use tags::compute_tag_specs;

use crate::structure::opaque_regions_from_tree;
use crate::validation::TemplateValidator;

/// Validate a Django template file.
///
/// This is a semantic convenience entrypoint: parsing still lives in
/// `djls-templates`, while this function triggers validation for callers that
/// need Django meaning for a file.
#[salsa::tracked]
pub fn validate_template_file(db: &dyn Db, file: djls_source::File) {
    let Some(nodelist) = djls_templates::parse_template(db, file) else {
        return;
    };

    validate_nodelist(db, nodelist);
}

/// Validate a Django template node list and return validation errors.
///
/// This function builds a `TemplateTree` from the parsed node list and, during
/// construction, accumulates semantic validation errors for issues such as:
/// - Unclosed block tags
/// - Mismatched tag pairs
/// - Orphaned intermediate tags
/// - Invalid argument counts
/// - Unmatched block names
#[salsa::tracked]
pub fn validate_nodelist(db: &dyn Db, nodelist: djls_templates::NodeList<'_>) {
    let nodes = nodelist.nodelist(db);
    if nodes.is_empty() {
        return;
    }

    // 1. Structural analysis accumulates block-structure diagnostics.
    let template_tree = build_template_tree(db, nodelist);

    // 2. Perform all other validations in a single walk.
    let opaque_regions = opaque_regions_from_tree(template_tree.regions(db));
    let validator = TemplateValidator::new(db, nodelist, &opaque_regions);
    validator.validate(nodes);
}

pub mod builtins;
pub mod db;
pub mod errors;
pub mod specs;
pub mod validation;

pub use builtins::django_builtin_specs;
pub use db::Db;
pub use db::ValidationErrorAccumulator;
pub use errors::ValidationError;
pub use specs::ArgType;
pub use specs::EndTag;
pub use specs::IntermediateTag;
pub use specs::SimpleArgType;
pub use specs::TagArg;
pub use specs::TagSpec;
pub use specs::TagSpecs;
pub use validation::TagValidator;

/// Validate a Django template node list and return validation errors.
///
/// This function runs the TagValidator on the parsed node list to check for:
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

    TagValidator::new(db, nodelist).validate();
}

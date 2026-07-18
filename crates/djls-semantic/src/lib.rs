mod db;
mod diagnostics;
mod errors;
mod filters;
mod inheritance;
mod offset;
mod references;
mod scoping;
mod structure;
mod tags;
mod validation;

pub use db::Db;
pub use db::ValidationErrorAccumulator;
pub use db::template_environment_for_file;
pub use diagnostics::TemplateDiagnostics;
pub use diagnostics::collect_template_diagnostics;
pub use djls_project::TagArgument;
pub use djls_project::TagArgumentKind;
pub use errors::ValidationError;
pub use filters::FilterAritySpecs;
pub use filters::LibraryFilterSpecs;
pub use filters::library_filter_specs;
pub use inheritance::BlockDef;
pub use inheritance::BlockSite;
pub use inheritance::ChainEnd;
pub use inheritance::ExtendsTarget;
pub use inheritance::PartialDef;
pub use inheritance::TemplateInheritance;
pub use inheritance::TemplateSymbols;
pub use inheritance::block_overrides;
pub use inheritance::inherited_blocks;
pub use inheritance::parent_block;
pub use inheritance::template_inheritance;
pub use inheritance::template_symbols;
pub use offset::SemanticOffsetContext;
pub use references::TemplateLibraryReferenceInFile;
pub use references::TemplateLibraryReferencesInFile;
pub use references::TemplateReference;
pub use references::TemplateReferenceInFile;
pub use references::TemplateReferenceKind;
pub use references::TemplateReferencesInFile;
pub use references::references_to_template_name;
pub use references::resolve_reference_for_file;
pub use references::resolve_reference_origins;
pub use references::template_library_references_in_file;
pub use references::template_references_in_file;
pub use scoping::effective_symbol_candidate_at;
pub use structure::BlockRole;
pub use structure::GrammarOpeningDefinition;
pub use structure::OpaqueRegions;
pub use structure::OutlineItem;
pub use structure::OutlineKind;
pub use structure::RegionId;
pub use structure::SemanticGrammarVocabulary;
pub use structure::TemplateFold;
pub use structure::TemplateFoldKind;
pub use structure::TemplateNode;
pub use structure::TemplateRegion;
pub use structure::TemplateTree;
pub use structure::build_template_folds;
pub use structure::build_template_outline_for_file;
pub use structure::build_template_tree_for_file;
pub use structure::compute_opaque_regions;
pub use structure::semantic_grammar_vocabulary;
pub use tags::EndTag;
pub use tags::IntermediateTag;
pub use tags::LibraryTagSpecs;
pub use tags::TagRole;
pub use tags::TagSpec;
pub use tags::TagSpecs;
pub use tags::builtin_tag_specs;
pub use tags::library_tag_specs;
pub use tags::tag_spec_at;
pub use tags::tag_specs_at;
pub use tags::tag_specs_for_file;

use crate::validation::TemplateValidator;

/// Validate a Django template file.
///
/// This is a semantic convenience entrypoint: parsing still lives in
/// `djls-templates`, while this function triggers validation for callers that
/// need Django meaning for a file.
#[salsa::tracked]
pub fn validate_template_file(db: &dyn Db, file: djls_source::File) {
    let djls_templates::TemplateParseResult::Parsed(nodelist) =
        djls_templates::parse_template(db, file)
    else {
        return;
    };

    let projection = crate::scoping::template_analysis_projection_for_file(db, file, nodelist);
    TemplateValidator::new(db, projection).validate();
}

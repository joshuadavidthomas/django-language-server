mod db;
mod diagnostics;
mod errors;
mod expressions;
mod filters;
mod inheritance;
mod layout;
mod offset;
mod references;
mod scoping;
mod structure;
mod tags;
mod validation;

pub use db::Db;
pub use db::ValidationErrorAccumulator;
pub use db::scoped_template_libraries_for_file;
pub use diagnostics::TemplateDiagnostics;
pub use diagnostics::collect_template_diagnostics;
pub use djls_project::ArgumentFormCoverage;
pub use djls_project::ParameterRequirement;
pub use djls_project::TagArgument;
pub use djls_project::TagArgumentForm;
pub use djls_project::TagArgumentKind;
pub use djls_project::TagArgumentSyntax;
use djls_source::File;
use djls_templates::TemplateParseResult;
use djls_templates::parse_template;
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
pub use inheritance::ancestor_blocks;
pub use inheritance::block_definition_at;
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
pub use references::TemplateReferencesInFile;
pub use references::references_to_template_name;
pub use references::resolve_reference_for_file;
pub use references::resolve_reference_origins;
pub use references::template_library_references_in_file;
pub use references::template_references_in_file;
pub use scoping::effective_symbol_candidate_at;
pub use structure::BlockRole;
pub use structure::GrammarOpeningDefinition;
pub use structure::OutlineItem;
pub use structure::OutlineKind;
pub use structure::RegionId;
pub use structure::SemanticGrammarVocabulary;
pub use structure::TemplateFold;
pub use structure::TemplateFoldKind;
pub use structure::TemplateNode;
pub use structure::TemplateRegion;
pub use structure::TemplateTree;
pub use structure::build_projectless_template_tree;
pub use structure::build_template_folds_for_file;
pub use structure::build_template_outline_for_file;
pub use structure::semantic_grammar_vocabulary;
pub use tags::BodyAnalysis;
pub use tags::EndTag;
pub use tags::IntermediateTag;
pub use tags::LibraryTagSpecs;
pub use tags::TagSpec;
pub use tags::TagSpecs;
pub use tags::bundled_django_tag_specs;
pub use tags::library_tag_specs;
pub use tags::tag_spec_at;
pub use tags::tag_specs_at_prefix;
pub use tags::tag_specs_for_file;

use crate::layout::TemplateLayout;
use crate::layout::template_layout;
use crate::scoping::analyze_template_layout_in_scope;
use crate::validation::TemplateValidator;

/// Validate a Django template file.
///
/// This is a semantic convenience entrypoint: parsing still lives in
/// `djls-templates`, while this function triggers validation for callers that
/// need Django meaning for a file.
#[salsa::tracked(returns(copy))]
pub fn validate_template_file(db: &dyn Db, file: File) {
    let TemplateParseResult::Parsed(nodelist) = parse_template(db, file) else {
        return;
    };

    let layout = template_layout(db, nodelist);
    let positions = layout.positions(db);
    for error in validate_template_layout(db, file, layout) {
        use salsa::Accumulator;

        ValidationErrorAccumulator(positions.resolve(error.clone())).accumulate(db);
    }
}

// Returned rather than accumulated: reading accumulated values inside a tracked
// function is an untracked read, which would re-run every file's validation on
// every revision.
#[salsa::tracked(returns(ref))]
fn validate_template_layout(
    db: &dyn Db,
    file: File,
    layout: TemplateLayout<'_>,
) -> Vec<ValidationError> {
    let projection = analyze_template_layout_in_scope(db, file, layout, file);
    let mut errors = TemplateValidator::new(db, projection).validate();
    // Validation's own errors come before its structural dependency's, as when both
    // were accumulated.
    errors.extend(projection.diagnostics(db).iter().cloned());
    errors
}

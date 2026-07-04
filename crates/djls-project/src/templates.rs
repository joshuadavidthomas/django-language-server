mod candidates;
mod filters;
mod libraries;
mod names;
mod registrations;
mod resolution;
mod symbols;
mod tags;

pub(crate) use candidates::discover_templatetag_candidate_paths;
pub use filters::FilterArity;
pub use filters::FilterArityMap;
pub use filters::extract_filter_arities;
pub use libraries::TemplateInventoryStatus;
pub use libraries::TemplateLibraries;
pub use libraries::TemplateLibrary;
pub use libraries::TemplateSymbolAvailability;
pub use libraries::TemplateSymbolCandidate;
pub use libraries::UnknownLibraryOutcome;
pub use libraries::UnknownSymbolOutcome;
pub use libraries::template_libraries;
pub use names::InvalidTemplateIdentifier;
pub use names::LibraryName;
pub use names::TemplateSymbolName;
pub(crate) use registrations::RegistrationKind;
pub(crate) use registrations::for_each_registration;
pub use resolution::FindTemplateResult;
pub use resolution::TemplateDoesNotExist;
pub use resolution::TemplateName;
pub use resolution::TemplateOrigin;
pub use resolution::TemplateResolution;
pub use resolution::TriedTemplateSource;
pub use resolution::resolve_relative_name;
pub use resolution::template_resolution;
pub use symbols::SymbolDefinition;
pub use symbols::SymbolKey;
pub use symbols::TemplateSymbol;
pub use symbols::TemplateSymbolKind;
pub use tags::ArgumentCountConstraint;
pub use tags::AsVar;
pub use tags::BlockSpec;
pub use tags::BlockSpecs;
pub use tags::ChoiceAt;
pub use tags::ExtractedDiagnosticConstraint;
pub use tags::ExtractedDiagnosticMessage;
pub use tags::ExtractedMessageArg;
pub use tags::ExtractedMessageTemplate;
pub use tags::KnownOptions;
pub use tags::RequiredKeyword;
pub use tags::SplitPosition;
pub use tags::TagArgument;
pub use tags::TagArgumentKind;
pub use tags::TagRule;
pub use tags::TagRuleMap;
pub use tags::extract_block_specs;
pub use tags::extract_tag_rules;

use crate::python::PythonModuleName;

fn guess_package_module_name_from_installed_app_entry(entry: &str) -> Option<PythonModuleName> {
    let module = if let Some((module, _)) = entry.split_once(".apps.") {
        module
    } else if entry.ends_with("Config") {
        entry.rsplit_once('.').map_or(entry, |(module, _)| module)
    } else {
        entry
    };

    PythonModuleName::parse(module).ok()
}

mod candidates;
pub(crate) mod filters;
mod libraries;
mod names;
mod registrations;
mod resolution;
mod symbols;
mod tags;

pub(crate) use candidates::refresh_templatetag_candidate_paths;
pub(crate) use candidates::templatetag_candidates;
pub(crate) use candidates::templatetag_package_candidates;
pub use filters::FilterArity;
pub use filters::FilterArityMap;
pub use filters::extract_filter_arities;
pub use libraries::BuiltinLibrarySource;
pub use libraries::InactiveTemplateLibrarySource;
pub use libraries::InstalledSymbolCandidate;
pub use libraries::InstalledSymbolOrigin;
pub use libraries::LoadableLibrarySource;
pub use libraries::ResolvedTemplateLibrary;
pub use libraries::TemplateLibraries;
pub use libraries::TemplateLibrariesBuilder;
pub use libraries::TemplateLibrary;
pub use libraries::TemplateLibraryId;
pub use libraries::TemplateLibraryResolution;
pub use libraries::TemplateLibraryResolutionError;
pub use libraries::TemplateLibraryStatus;
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

fn guess_package_module_from_installed_app_entry(entry: &str) -> &str {
    if let Some((module, _)) = entry.split_once(".apps.") {
        module
    } else if entry.ends_with("Config") {
        entry.rsplit_once('.').map_or(entry, |(module, _)| module)
    } else {
        entry
    }
}

mod candidates;
mod dirs;
pub(crate) mod filters;
mod inactive;
mod libraries;
mod names;
mod origins;
mod registrations;
mod symbols;
mod tags;

pub(crate) use candidates::TemplateTagCandidate;
pub(crate) use candidates::refresh_templatetag_candidate_paths;
pub(crate) use candidates::templatetag_candidates;
pub(crate) use candidates::templatetag_package_candidates;
pub use dirs::template_dirs;
pub use filters::FilterArity;
pub use filters::FilterArityMap;
pub use filters::extract_filter_arities;
pub use inactive::InactiveLibraries;
pub use inactive::InactiveLibrary;
pub use inactive::inactive_template_libraries;
pub use libraries::InstalledSymbolCandidate;
pub use libraries::InstalledSymbolOrigin;
pub use libraries::TemplateLibraries;
pub use libraries::TemplateLibrary;
pub use libraries::template_libraries;
pub use names::InvalidTemplateIdentifier;
pub use names::LibraryName;
pub use names::TemplateSymbolName;
pub use origins::FindTemplateResult;
pub use origins::ProjectTemplateFile;
pub use origins::ProjectTemplateFiles;
pub use origins::TemplateDoesNotExist;
pub use origins::TemplateName;
pub use origins::TemplateOrigin;
pub use origins::TemplateOrigins;
pub use origins::TriedTemplateSource;
pub use origins::find_template;
pub use origins::project_template_files;
pub use origins::template_origins;
pub(crate) use registrations::RegistrationKind;
pub(crate) use registrations::TemplateLibraryAnalysis;
pub(crate) use registrations::for_each_registration;
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

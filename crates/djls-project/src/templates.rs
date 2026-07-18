mod candidates;
mod configurations;
mod environment;
mod filters;
mod libraries;
mod names;
mod registrations;
mod resolution;
mod symbols;
mod tags;

pub(crate) use candidates::discover_templatetag_candidate_paths;
pub use environment::TemplateEnvironment;
pub use environment::template_environment;
pub use filters::FilterArity;
pub use filters::FilterArityMap;
pub use libraries::AvailableAppCandidates;
pub use libraries::ContextualLibraryChain;
pub use libraries::ContextualLibraryStep;
pub use libraries::EffectiveDefinitionLibrary;
pub use libraries::EnvironmentSymbolLookup;
pub use libraries::LoadableLibraryLookup;
pub use libraries::MissingLibraryLookup;
pub use libraries::TemplateLibraries;
pub use libraries::TemplateLibrary;
pub use libraries::TemplateLibraryKey;
pub use libraries::TemplateSymbolAvailability;
pub use libraries::TemplateSymbolCandidate;
pub use libraries::TemplateSymbolLookup;
pub use libraries::template_libraries;
pub use names::InvalidTemplateIdentifier;
pub use names::LibraryName;
pub use names::TemplateSymbolName;
pub(crate) use registrations::RegistrationKind;
pub use registrations::TemplateLibraryDefinitionFacts;
pub use registrations::TemplateLibraryFilterFacts;
pub use registrations::TemplateLibraryTagFacts;
pub use registrations::template_library_definition_facts;
pub use registrations::template_library_filter_facts;
pub use registrations::template_library_tag_facts;
pub use resolution::FindTemplateResult;
pub use resolution::InconclusiveTemplateSearch;
pub use resolution::ScopedTemplateReferenceResolution;
pub use resolution::TemplateBackendScope;
pub use resolution::TemplateDirectories;
pub use resolution::TemplateDoesNotExist;
pub use resolution::TemplateName;
pub use resolution::TemplateOrigin;
pub use resolution::TemplateResolution;
pub use resolution::resolve_relative_name;
pub use resolution::template_directories;
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

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModuleName;
use crate::python::resolve_package_dirs;
use crate::python::resolve_prefix;

fn installed_app_package_module(
    db: &dyn ProjectDb,
    project: Project,
    entry: &str,
) -> Option<PythonModuleName> {
    let resolved = resolve_prefix(db, project, entry);
    let Some(module) = resolved.module else {
        let name = PythonModuleName::parse(entry).ok()?;
        let package_dirs = resolve_package_dirs(db, project, name.clone());
        return (!package_dirs.dirs.is_empty()).then_some(name);
    };

    match resolved.unresolved_tail.len() {
        0 => Some(module.name().clone()),
        1 if module.path().file_name() == Some("__init__.py") => Some(module.name().clone()),
        1 => module
            .name()
            .as_str()
            .rsplit_once('.')
            .and_then(|(parent, _)| PythonModuleName::parse(parent).ok()),
        _ => None,
    }
}

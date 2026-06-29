mod ast;
mod db;
mod models;
mod parse;
mod project;
mod python;
mod refresh;
mod resolve;
mod settings;
mod templates;

pub use db::Db;
pub use models::ModelGraph;
pub use models::ModelId;
pub use models::compute_model_graph;
pub use project::Project;
pub use python::Interpreter;
pub use python::InvalidModulePath;
pub use python::PythonModule;
pub use python::PythonModulePath;
pub use refresh::RefreshCountUnits;
pub use refresh::RefreshData;
pub use refresh::RefreshPart;
pub use refresh::RefreshTask;
pub use refresh::RefreshTaskDescriptor;
pub use refresh::RefreshTaskGroup;
pub use refresh::apply_refresh;
pub use refresh::refresh_tasks;
pub use resolve::SearchPath;
pub use resolve::SearchPaths;
pub use resolve::model_modules;
pub use settings::StaticKnowledge;
pub use templates::ArgumentCountConstraint;
pub use templates::AsVar;
pub use templates::BlockSpec;
pub use templates::BlockSpecs;
pub use templates::BuiltinLibrarySource;
pub use templates::ChoiceAt;
pub use templates::ExtractedDiagnosticConstraint;
pub use templates::ExtractedDiagnosticMessage;
pub use templates::ExtractedMessageArg;
pub use templates::ExtractedMessageTemplate;
pub use templates::FilterArity;
pub use templates::FilterArityMap;
pub use templates::FindTemplateResult;
pub use templates::InactiveTemplateLibrarySource;
pub use templates::InstalledSymbolCandidate;
pub use templates::InstalledSymbolOrigin;
pub use templates::InvalidTemplateIdentifier;
pub use templates::KnownOptions;
pub use templates::LibraryName;
pub use templates::LoadableLibrarySource;
pub use templates::RequiredKeyword;
pub use templates::ResolvedTemplateLibrary;
pub use templates::SplitPosition;
pub use templates::SymbolDefinition;
pub use templates::SymbolKey;
pub use templates::TagArgument;
pub use templates::TagArgumentKind;
pub use templates::TagRule;
pub use templates::TagRuleMap;
pub use templates::TemplateDoesNotExist;
pub use templates::TemplateLibraries;
pub use templates::TemplateLibrary;
pub use templates::TemplateLibraryId;
pub use templates::TemplateLibraryResolution;
pub use templates::TemplateLibraryResolutionError;
pub use templates::TemplateLibraryStatus;
pub use templates::TemplateName;
pub use templates::TemplateOrigin;
pub use templates::TemplateResolution;
pub use templates::TemplateSymbol;
pub use templates::TemplateSymbolKind;
pub use templates::TemplateSymbolName;
pub use templates::TriedTemplateSource;
pub use templates::UnknownLibraryOutcome;
pub use templates::UnknownSymbolOutcome;
pub use templates::extract_block_specs;
pub use templates::extract_filter_arities;
pub use templates::extract_tag_rules;
pub use templates::template_libraries;
pub use templates::template_resolution;

// Test and benchmark support only; not part of the stable Project Facts façade.
#[doc(hidden)]
pub mod testing {
    pub use crate::models::extract_model_graph;
    pub use crate::refresh::compute_refresh;
    pub use crate::resolve::model_modules;
    pub use crate::settings::StaticKnowledge;

    use crate::templates::BuiltinLibraryPart;
    use crate::templates::BuiltinLibrarySource;
    use crate::templates::InactiveLibraryPart;
    use crate::templates::LoadableLibraryPart;
    use crate::templates::LoadableLibrarySource;

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct BuiltinInput {
        pub module: super::PythonModulePath,
        pub symbols: Vec<super::TemplateSymbol>,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct LoadableInput {
        pub load_name: super::LibraryName,
        pub module: super::PythonModulePath,
        pub symbols: Vec<super::TemplateSymbol>,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct InactiveInput {
        pub load_name: super::LibraryName,
        pub app: super::PythonModulePath,
        pub module: super::PythonModulePath,
        pub symbols: Vec<super::TemplateSymbol>,
    }

    #[must_use]
    pub fn template_libraries(
        knowledge: StaticKnowledge,
        builtins: Vec<BuiltinInput>,
        loadables: Vec<LoadableInput>,
        inactives: Vec<InactiveInput>,
    ) -> super::TemplateLibraries {
        let builtins = builtins
            .into_iter()
            .map(|input| BuiltinLibraryPart {
                module: input.module,
                source: BuiltinLibrarySource::DjangoDefault,
                symbols: input.symbols,
            })
            .collect();
        let loadables = loadables
            .into_iter()
            .map(|input| LoadableLibraryPart {
                load_name: input.load_name,
                module: input.module,
                source: LoadableLibrarySource::ConfiguredAlias,
                symbols: input.symbols,
            })
            .collect();
        let inactives = inactives
            .into_iter()
            .map(|input| InactiveLibraryPart {
                load_name: input.load_name,
                app: input.app,
                module: input.module,
                symbols: input.symbols,
            })
            .collect();

        super::TemplateLibraries::from_parts(knowledge, builtins, loadables, inactives)
    }
}

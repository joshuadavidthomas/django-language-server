mod ast;
mod db;
mod discovery;
mod models;
mod project;
mod python;
mod settings;
mod templates;

pub use db::Db;
pub use discovery::CountLabel;
pub use discovery::DiscoveryPhase;
pub use discovery::DjangoDiscoveryData;
pub use discovery::DjangoDiscoveryPart;
pub use discovery::DjangoDiscoveryProgress;
pub use discovery::EnvironmentPhase;
pub use discovery::ProjectFactsPhase;
pub use discovery::apply_django_discovery;
pub use discovery::django_discovery_phases;
pub use models::ModelGraph;
pub use models::ModelId;
pub use models::compute_model_graph;
pub use project::Project;
pub use python::CandidateKind;
pub use python::CandidateLocation;
pub use python::FileModuleDerivation;
pub use python::FileModuleDerivationStatus;
pub use python::FileResolutionDetail;
pub use python::FileUnresolvedReason;
pub use python::Interpreter;
pub use python::InvalidModuleName;
pub use python::PackageDirs;
pub use python::PythonModule;
pub use python::PythonModuleName;
pub use python::ResolutionDetail;
pub use python::ResolvedPrefix;
pub use python::SearchPath;
pub use python::SearchPaths;
pub use python::UnresolvedReason;
pub use python::file_to_module;
pub use python::file_to_module_detail;
pub use python::resolve_module_detail;
pub use python::resolve_package_dirs;
pub use python::resolve_prefix;
pub use templates::ArgumentCountConstraint;
pub use templates::AsVar;
pub use templates::BlockSpec;
pub use templates::BlockSpecs;
pub use templates::ChoiceAt;
pub use templates::ExtractedDiagnosticConstraint;
pub use templates::ExtractedDiagnosticMessage;
pub use templates::ExtractedMessageArg;
pub use templates::ExtractedMessageTemplate;
pub use templates::FilterArity;
pub use templates::FilterArityMap;
pub use templates::FindTemplateResult;
pub use templates::InvalidTemplateIdentifier;
pub use templates::KnownOptions;
pub use templates::LibraryName;
pub use templates::RequiredKeyword;
pub use templates::SplitPosition;
pub use templates::SymbolDefinition;
pub use templates::SymbolKey;
pub use templates::TagArgument;
pub use templates::TagArgumentKind;
pub use templates::TagRule;
pub use templates::TagRuleMap;
pub use templates::TemplateDoesNotExist;
pub use templates::TemplateInventoryStatus;
pub use templates::TemplateLibraries;
pub use templates::TemplateLibrary;
pub use templates::TemplateName;
pub use templates::TemplateOrigin;
pub use templates::TemplateResolution;
pub use templates::TemplateSymbol;
pub use templates::TemplateSymbolAvailability;
pub use templates::TemplateSymbolCandidate;
pub use templates::TemplateSymbolKind;
pub use templates::TemplateSymbolName;
pub use templates::TriedTemplateSource;
pub use templates::UnknownLibraryOutcome;
pub use templates::UnknownSymbolOutcome;
pub use templates::extract_block_specs;
pub use templates::extract_filter_arities;
pub use templates::extract_tag_rules;
pub use templates::resolve_relative_name;
pub use templates::template_libraries;
pub use templates::template_resolution;

// Test and benchmark support only; not part of the stable Project Facts façade.
#[doc(hidden)]
pub mod testing {
    use camino::Utf8PathBuf;

    pub use crate::discovery::compute_django_discovery;
    pub use crate::models::model_modules;
    pub use crate::models::resolve_model_graph_from_modules;

    pub fn extract_model_graph(
        db: &dyn djls_source::Db,
        file: djls_source::File,
        module_name: super::PythonModuleName,
    ) -> &super::ModelGraph {
        crate::models::extract_models(db, file, module_name).graph()
    }

    pub fn settings_module_file(
        db: &dyn super::Db,
        project: super::Project,
    ) -> Option<djls_source::File> {
        crate::settings::settings_module_file(db, project)
    }

    pub fn django_settings_template_backend_count(
        db: &dyn super::Db,
        project: super::Project,
    ) -> usize {
        crate::settings::django_settings(db, project)
            .templates
            .backends
            .len()
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum TemplateLibraryInput {
        Builtin {
            module: super::PythonModuleName,
            symbols: Vec<super::TemplateSymbol>,
        },
        Installed {
            load_name: super::LibraryName,
            module: super::PythonModuleName,
            symbols: Vec<super::TemplateSymbol>,
        },
        Available {
            load_name: super::LibraryName,
            app: super::PythonModuleName,
            module: super::PythonModuleName,
            symbols: Vec<super::TemplateSymbol>,
        },
    }

    #[must_use]
    pub fn template_libraries(
        db: &dyn super::Db,
        status: super::TemplateInventoryStatus,
        inputs: Vec<TemplateLibraryInput>,
    ) -> super::TemplateLibraries {
        let libraries = inputs
            .into_iter()
            .map(|input| match input {
                TemplateLibraryInput::Builtin { module, symbols } => {
                    super::TemplateLibrary::builtin(testing_module(db, module), symbols)
                }
                TemplateLibraryInput::Installed {
                    load_name,
                    module,
                    symbols,
                } => super::TemplateLibrary::installed(
                    load_name,
                    testing_module(db, module),
                    symbols,
                ),
                TemplateLibraryInput::Available {
                    load_name,
                    app,
                    module,
                    symbols,
                } => super::TemplateLibrary::available(
                    load_name,
                    app,
                    testing_module(db, module),
                    symbols,
                ),
            })
            .collect();

        super::TemplateLibraries::from_libraries(status, libraries)
    }

    fn testing_module(
        db: &dyn super::Db,
        module_name: super::PythonModuleName,
    ) -> super::PythonModule {
        let path = Utf8PathBuf::from(format!(
            "/__djls_testing__/{}.py",
            module_name.as_str().replace('.', "/")
        ));
        let file = db.get_or_create_file(&path);
        super::PythonModule::new(module_name, path, file)
    }
}

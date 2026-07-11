use serde::Serialize;

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
pub use discovery::DjangoDiscoveryProgress;
pub use discovery::DjangoEnvironmentData;
pub use discovery::EnvironmentPart;
pub use discovery::EnvironmentPhase;
pub use discovery::ProjectFactsData;
pub use discovery::ProjectFactsPart;
pub use discovery::ProjectFactsPhase;
pub use discovery::apply_django_environment;
pub use discovery::environment_phases;
pub use discovery::project_facts_phases;
pub use models::ModelGraph;
pub use models::ModelId;
pub use models::compute_model_graph;
pub use project::Project;
pub use python::FileModuleCandidate;
pub use python::FileModuleResolution;
pub use python::Interpreter;
pub use python::InvalidModuleName;
pub use python::PackageDirs;
pub use python::PythonModule;
pub use python::PythonModuleName;
pub use python::ResolvedPrefix;
pub use python::SearchPath;
pub use python::SearchPaths;
pub use python::file_to_module;
pub use python::file_to_module_resolution;
pub use python::resolve_package_dirs;
pub use python::resolve_prefix;
pub use templates::ArgumentCountConstraint;
pub use templates::AsVar;
pub use templates::AvailableAppCandidates;
pub use templates::BlockSpec;
pub use templates::BlockSpecs;
pub use templates::ChoiceAt;
pub use templates::ExtractedDiagnosticConstraint;
pub use templates::ExtractedDiagnosticMessage;
pub use templates::ExtractedMessageArg;
pub use templates::ExtractedMessageTemplate;
pub use templates::FilterArity;
pub use templates::FilterArityExtraction;
pub use templates::FilterArityMap;
pub use templates::FindTemplateResult;
pub use templates::InconclusiveTemplateSearch;
pub use templates::InvalidTemplateIdentifier;
pub use templates::KnownOptions;
pub use templates::LibraryName;
pub use templates::LoadableLibraryLookup;
pub use templates::MissingLibraryLookup;
pub use templates::RequiredKeyword;
pub use templates::SplitPosition;
pub use templates::SymbolDefinition;
pub use templates::SymbolKey;
pub use templates::TagArgument;
pub use templates::TagArgumentKind;
pub use templates::TagRule;
pub use templates::TagRuleMap;
pub use templates::TemplateContextProcessor;
pub use templates::TemplateContextProcessors;
pub use templates::TemplateDirectories;
pub use templates::TemplateDoesNotExist;
pub use templates::TemplateLibraries;
pub use templates::TemplateLibrary;
pub use templates::TemplateName;
pub use templates::TemplateOrigin;
pub use templates::TemplateResolution;
pub use templates::TemplateSymbol;
pub use templates::TemplateSymbolAvailability;
pub use templates::TemplateSymbolCandidate;
pub use templates::TemplateSymbolKind;
pub use templates::TemplateSymbolLookup;
pub use templates::TemplateSymbolName;
pub use templates::extract_block_specs;
pub use templates::extract_filter_arities;
pub use templates::extract_tag_rules;
pub use templates::resolve_relative_name;
pub use templates::template_context_processors;
pub use templates::template_directories;
pub use templates::template_libraries;
pub use templates::template_resolution;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ExtractionStatus {
    Complete,
    Partial,
}

// Test and benchmark support only; not part of the stable Project Facts façade.
#[doc(hidden)]
pub mod testing {
    use camino::Utf8PathBuf;
    use djls_source::File;
    use djls_source::FileStatus;
    use djls_source::Span;

    pub use crate::discovery::compute_django_environment;
    pub use crate::discovery::compute_project_facts;
    pub use crate::models::model_modules;
    pub use crate::models::resolve_model_graph_from_modules;
    pub use crate::python::PythonSyntaxError;
    pub use crate::python::PythonSyntaxErrorClass;

    pub fn python_syntax_errors(
        db: &dyn djls_source::Db,
        file: djls_source::File,
    ) -> Option<Vec<PythonSyntaxError>> {
        crate::python::python_syntax_errors(db, file).cloned()
    }

    pub fn extract_model_graph(
        db: &dyn djls_source::Db,
        file: djls_source::File,
        module_name: super::PythonModuleName,
    ) -> &super::ModelGraph {
        crate::models::extract_models(db, file, module_name).graph()
    }

    #[must_use]
    pub fn model_location(
        graph: &super::ModelGraph,
        module_name: &str,
        model_name: &str,
    ) -> Option<(File, Span)> {
        graph
            .models_named(model_name)
            .find(|(id, _model)| id.module_name().as_str() == module_name)
            .map(|(_id, model)| (model.file, model.name.span()))
    }

    #[must_use]
    pub fn model_relation_locations(
        graph: &super::ModelGraph,
        module_name: &str,
        model_name: &str,
    ) -> Vec<(String, File, Span, Option<Span>)> {
        graph
            .models_named(model_name)
            .find(|(id, _model)| id.module_name().as_str() == module_name)
            .map(|(_id, model)| {
                model
                    .relations
                    .iter()
                    .map(|relation| {
                        (
                            relation.field_name.value().as_str().to_string(),
                            relation.file,
                            relation.field_name.span(),
                            relation.target_span(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn settings_module_file(
        db: &dyn super::Db,
        project: super::Project,
    ) -> Option<djls_source::File> {
        crate::settings::settings_module_file(db, project)
    }

    pub fn django_settings(
        db: &dyn super::Db,
        project: super::Project,
    ) -> impl serde::Serialize + '_ {
        crate::settings::django_settings(db, project)
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct TemplateBackendLibrariesInput {
        pub loadable: Vec<(super::LibraryName, super::PythonModuleName)>,
        pub builtins: Vec<super::PythonModuleName>,
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
        inputs: Vec<TemplateLibraryInput>,
    ) -> super::TemplateLibraries {
        build_template_libraries(db, inputs, false)
    }

    #[must_use]
    pub fn template_libraries_with_omissions(
        db: &dyn super::Db,
        inputs: Vec<TemplateLibraryInput>,
    ) -> super::TemplateLibraries {
        build_template_libraries(db, inputs, true)
    }

    fn build_template_libraries(
        db: &dyn super::Db,
        inputs: Vec<TemplateLibraryInput>,
        has_omissions: bool,
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

        if has_omissions {
            super::TemplateLibraries::from_libraries_with_omissions(libraries)
        } else {
            super::TemplateLibraries::from_libraries(libraries)
        }
    }

    #[must_use]
    pub fn template_libraries_with_configurations(
        db: &dyn super::Db,
        inputs: Vec<TemplateLibraryInput>,
        configurations: Vec<Vec<TemplateBackendLibrariesInput>>,
    ) -> super::TemplateLibraries {
        configure_template_libraries(template_libraries(db, inputs), configurations)
    }

    #[must_use]
    pub fn template_libraries_with_configuration_omissions(
        db: &dyn super::Db,
        inputs: Vec<TemplateLibraryInput>,
        configurations: Vec<Vec<TemplateBackendLibrariesInput>>,
    ) -> super::TemplateLibraries {
        configure_template_libraries(
            template_libraries_with_omissions(db, inputs),
            configurations,
        )
    }

    fn configure_template_libraries(
        mut libraries: super::TemplateLibraries,
        configurations: Vec<Vec<TemplateBackendLibrariesInput>>,
    ) -> super::TemplateLibraries {
        libraries.set_testing_configurations(
            configurations
                .into_iter()
                .map(|backends| {
                    backends
                        .into_iter()
                        .map(|backend| (backend.loadable, backend.builtins))
                        .collect()
                })
                .collect(),
        );
        libraries
    }

    fn testing_module(
        db: &dyn super::Db,
        module_name: super::PythonModuleName,
    ) -> super::PythonModule {
        let path = Utf8PathBuf::from(format!(
            "/__djls_testing__/{}.py",
            module_name.as_str().replace('.', "/")
        ));
        let file = File::builder(path.clone(), 0, FileStatus::Exists)
            .durability(salsa::Durability::LOW)
            .path_durability(salsa::Durability::HIGH)
            .new(db);
        db.files().register_file(db, file);
        super::PythonModule::new(
            module_name,
            path,
            file,
            super::SearchPath::FirstParty(Utf8PathBuf::from("/__djls_testing__")),
        )
    }
}

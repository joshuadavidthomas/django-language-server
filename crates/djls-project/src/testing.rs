mod python_evaluation;

use djls_source::Db as SourceDb;
use djls_source::File;
use djls_source::Span;
pub use python_evaluation::PythonBindingAlternativeView;
pub use python_evaluation::PythonBindingView;
pub use python_evaluation::PythonBoundValueView;
pub use python_evaluation::PythonDictItemView;
pub use python_evaluation::PythonFileReadErrorView;
pub use python_evaluation::PythonImportNameErrorView;
pub use python_evaluation::PythonImportOutcomeView;
pub use python_evaluation::PythonIntrinsicView;
pub use python_evaluation::PythonModuleEvaluationError;
pub use python_evaluation::PythonModuleEvaluationView;
pub use python_evaluation::PythonModuleView;
pub use python_evaluation::PythonMutationOperationView;
pub use python_evaluation::PythonMutationPathSegmentView;
pub use python_evaluation::PythonMutationView;
pub use python_evaluation::PythonSequenceItemView;
pub use python_evaluation::PythonUnknownCauseView;
pub use python_evaluation::PythonUnknownView;
pub use python_evaluation::PythonValueKindView;
pub use python_evaluation::PythonValueView;
pub use python_evaluation::python_module_evaluation;
pub use python_evaluation::python_module_evaluation_for_module;
use serde::Serialize;

use crate::db::Db;
pub use crate::discovery::compute_django_environment;
pub use crate::discovery::compute_project_facts;
use crate::models::AncestryOutcome;
use crate::models::BaseOutcome;
use crate::models::BaseUnresolvedReason;
use crate::models::ClassId;
use crate::models::InvalidAncestryReason;
use crate::models::ModelGraph;
use crate::models::ModelId;
use crate::models::extract_models;
pub use crate::models::model_modules;
use crate::models::resolve_local_model_graph;
pub use crate::models::resolve_model_graph_from_modules;
use crate::project::Project;
use crate::python::PythonModuleName;
pub use crate::python::PythonSyntaxError;
pub use crate::python::PythonSyntaxErrorClass;
use crate::python::python_syntax_errors as project_python_syntax_errors;
use crate::settings::django_settings as project_django_settings;
use crate::settings::settings_module_file as project_settings_module_file;
use crate::templates::LibraryName;
use crate::templates::TemplateLibrary;
use crate::templates::TemplateLibraryCatalog;
pub use crate::templates::TemplateLibraryFixtureError;
use crate::templates::TemplateLibraryId;
use crate::templates::TemplateSymbol;

pub fn python_syntax_errors(db: &dyn SourceDb, file: File) -> Option<Vec<PythonSyntaxError>> {
    project_python_syntax_errors(db, file).map(<[PythonSyntaxError]>::to_vec)
}

pub fn extract_model_graph(
    db: &dyn SourceDb,
    file: File,
    module_name: PythonModuleName,
) -> ModelGraph {
    resolve_local_model_graph(extract_models(db, file, module_name))
}

#[must_use]
pub fn model_location(
    graph: &ModelGraph,
    module_name: &str,
    model_name: &str,
) -> Option<(File, Span)> {
    graph
        .models_named(model_name)
        .find(|(id, _model)| id.module_name().as_str() == module_name)
        .map(|(_id, model)| (model.file, model.name.span()))
}

/// The import identity of an extracted Python class without claiming that the
/// resolver admitted it as a Django model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelClassIdentityView {
    module_name: PythonModuleName,
    name: String,
}

impl ModelClassIdentityView {
    #[must_use]
    pub fn module_name(&self) -> &PythonModuleName {
        &self.module_name
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl From<&ClassId> for ModelClassIdentityView {
    fn from(class: &ClassId) -> Self {
        Self {
            module_name: class.module_name().clone(),
            name: class.name().to_string(),
        }
    }
}

#[must_use]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelBaseOutcomeView {
    DjangoModelRoot {
        span: Span,
    },
    Model {
        model: ModelId,
        span: Span,
    },
    NonModelClass {
        class: ModelClassIdentityView,
        span: Span,
    },
    Unresolved {
        span: Span,
        reason: ModelBaseUnresolvedReasonView,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelBaseUnresolvedReasonView {
    UnsupportedExpression,
    MissingImportBinding {
        path: Vec<String>,
    },
    ShadowedImportBinding {
        path: Vec<String>,
    },
    InvalidImportedTarget {
        target: String,
    },
    ImportNotFound {
        requested: PythonModuleName,
    },
    ImportedTargetIsModule {
        module: PythonModuleName,
    },
    PartialImport {
        resolved_prefix: PythonModuleName,
        unresolved_tail: Vec<String>,
    },
    ClassNotFound {
        class: ModelClassIdentityView,
    },
    ReboundLocalBase {
        class: ModelClassIdentityView,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelMroEntryView {
    Model(ModelId),
    NonModelClass(ModelClassIdentityView),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelAncestryOutcomeView {
    Complete {
        mro: Vec<ModelMroEntryView>,
    },
    Partial,
    Invalid {
        reason: ModelInvalidAncestryReasonView,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelInvalidAncestryReasonView {
    Cycle,
    DuplicateDjangoModelRoot,
    DuplicateClassBase { class: ModelClassIdentityView },
    InconsistentMethodResolutionOrder,
}

fn model_base_unresolved_reason_view(
    reason: &BaseUnresolvedReason,
) -> ModelBaseUnresolvedReasonView {
    match reason {
        BaseUnresolvedReason::UnsupportedExpression => {
            ModelBaseUnresolvedReasonView::UnsupportedExpression
        }
        BaseUnresolvedReason::MissingImportBinding { path } => {
            ModelBaseUnresolvedReasonView::MissingImportBinding { path: path.clone() }
        }
        BaseUnresolvedReason::ShadowedImportBinding { path } => {
            ModelBaseUnresolvedReasonView::ShadowedImportBinding { path: path.clone() }
        }
        BaseUnresolvedReason::InvalidImportedTarget { target } => {
            ModelBaseUnresolvedReasonView::InvalidImportedTarget {
                target: target.clone(),
            }
        }
        BaseUnresolvedReason::ImportNotFound { requested } => {
            ModelBaseUnresolvedReasonView::ImportNotFound {
                requested: requested.clone(),
            }
        }
        BaseUnresolvedReason::ImportedTargetIsModule { module } => {
            ModelBaseUnresolvedReasonView::ImportedTargetIsModule {
                module: module.clone(),
            }
        }
        BaseUnresolvedReason::PartialImport {
            resolved_prefix,
            unresolved_tail,
        } => ModelBaseUnresolvedReasonView::PartialImport {
            resolved_prefix: resolved_prefix.clone(),
            unresolved_tail: unresolved_tail.clone(),
        },
        BaseUnresolvedReason::ClassNotFound { class } => {
            ModelBaseUnresolvedReasonView::ClassNotFound {
                class: class.into(),
            }
        }
        BaseUnresolvedReason::ReboundLocalBase { class } => {
            ModelBaseUnresolvedReasonView::ReboundLocalBase {
                class: class.into(),
            }
        }
    }
}

fn model_base_outcome_view(base: &BaseOutcome) -> ModelBaseOutcomeView {
    match base {
        BaseOutcome::DjangoModelRoot { .. } => {
            ModelBaseOutcomeView::DjangoModelRoot { span: base.span() }
        }
        BaseOutcome::Model { model, .. } => ModelBaseOutcomeView::Model {
            model: model.clone(),
            span: base.span(),
        },
        BaseOutcome::NonModelClass { class, .. } => ModelBaseOutcomeView::NonModelClass {
            class: class.into(),
            span: base.span(),
        },
        BaseOutcome::Unresolved { reason, .. } => ModelBaseOutcomeView::Unresolved {
            span: base.span(),
            reason: model_base_unresolved_reason_view(reason),
        },
    }
}

fn model_mro_entry_view(graph: &ModelGraph, class: &ClassId) -> ModelMroEntryView {
    graph
        .models_named(class.name())
        .find(|(model, _definition)| ClassId::from_model_id(model) == *class)
        .map_or_else(
            || ModelMroEntryView::NonModelClass(class.into()),
            |(model, _definition)| ModelMroEntryView::Model(model.clone()),
        )
}

fn model_ancestry_outcome_view(
    graph: &ModelGraph,
    ancestry: &AncestryOutcome,
) -> ModelAncestryOutcomeView {
    match ancestry {
        AncestryOutcome::Complete { mro } => ModelAncestryOutcomeView::Complete {
            mro: mro
                .iter()
                .map(|class| model_mro_entry_view(graph, class))
                .collect(),
        },
        AncestryOutcome::Partial => ModelAncestryOutcomeView::Partial,
        AncestryOutcome::Invalid { reason } => ModelAncestryOutcomeView::Invalid {
            reason: match reason {
                InvalidAncestryReason::Cycle => ModelInvalidAncestryReasonView::Cycle,
                InvalidAncestryReason::DuplicateDjangoModelRoot => {
                    ModelInvalidAncestryReasonView::DuplicateDjangoModelRoot
                }
                InvalidAncestryReason::DuplicateClassBase { class } => {
                    ModelInvalidAncestryReasonView::DuplicateClassBase {
                        class: class.into(),
                    }
                }
                InvalidAncestryReason::InconsistentMethodResolutionOrder => {
                    ModelInvalidAncestryReasonView::InconsistentMethodResolutionOrder
                }
            },
        },
    }
}

#[must_use]
pub fn model_inheritance_outcomes(
    graph: &ModelGraph,
    module_name: &str,
    model_name: &str,
) -> Option<(Vec<ModelBaseOutcomeView>, ModelAncestryOutcomeView)> {
    let (id, _model) = graph
        .models_named(model_name)
        .find(|(id, _model)| id.module_name().as_str() == module_name)?;
    let record = graph.inheritance(id)?;
    Some((
        record.bases.iter().map(model_base_outcome_view).collect(),
        model_ancestry_outcome_view(graph, &record.ancestry),
    ))
}

#[must_use]
pub fn model_relation_locations(
    graph: &ModelGraph,
    module_name: &str,
    model_name: &str,
) -> Vec<(String, File, Span, Option<Span>)> {
    graph
        .models_named(model_name)
        .find(|(id, _model)| id.module_name().as_str() == module_name)
        .map(|(id, _model)| {
            graph
                .owned_relation_entries(id)
                .map(|(relation, _resolution)| {
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

pub fn settings_module_file(db: &dyn Db, project: Project) -> Option<File> {
    project_settings_module_file(db, project)
}

pub fn django_settings(db: &dyn Db, project: Project) -> impl Serialize + '_ {
    project_django_settings(db, project)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateBackendLibrariesInput {
    pub loadable: Vec<(LibraryName, PythonModuleName)>,
    pub builtins: Vec<PythonModuleName>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateLibraryInput {
    Builtin {
        module: PythonModuleName,
        symbols: Vec<TemplateSymbol>,
    },
    Loadable {
        load_name: LibraryName,
        module: PythonModuleName,
        symbols: Vec<TemplateSymbol>,
    },
    AvailableInApp {
        load_name: LibraryName,
        app: PythonModuleName,
        module: PythonModuleName,
        symbols: Vec<TemplateSymbol>,
    },
}

#[must_use]
pub fn template_library_catalog(
    db: &dyn Db,
    inputs: Vec<TemplateLibraryInput>,
) -> TemplateLibraryCatalog {
    build_template_library_catalog(db, inputs, false)
}

#[must_use]
pub fn template_library_catalog_with_omissions(
    db: &dyn Db,
    inputs: Vec<TemplateLibraryInput>,
) -> TemplateLibraryCatalog {
    build_template_library_catalog(db, inputs, true)
}

fn build_template_library_catalog(
    db: &dyn Db,
    inputs: Vec<TemplateLibraryInput>,
    has_omissions: bool,
) -> TemplateLibraryCatalog {
    let libraries = inputs
        .into_iter()
        .map(|input| match input {
            TemplateLibraryInput::Builtin { module, symbols } => {
                let key = TemplateLibraryId::new(db, None, module.clone());
                TemplateLibrary::configured_builtin(key, module, symbols)
            }
            TemplateLibraryInput::Loadable {
                load_name,
                module,
                symbols,
            } => {
                let key = TemplateLibraryId::new(db, None, module.clone());
                TemplateLibrary::configured_loadable(key, load_name, module, symbols)
            }
            TemplateLibraryInput::AvailableInApp {
                load_name,
                app,
                module,
                symbols,
            } => {
                let key = TemplateLibraryId::new(db, None, module.clone());
                TemplateLibrary::configured_available_in_app(key, load_name, app, module, symbols)
            }
        })
        .collect();

    if has_omissions {
        TemplateLibraryCatalog::from_libraries_with_omissions(libraries)
    } else {
        TemplateLibraryCatalog::from_libraries(libraries)
    }
}

pub fn template_library_catalog_with_settings_cases(
    db: &dyn Db,
    inputs: Vec<TemplateLibraryInput>,
    settings_cases: Vec<Vec<TemplateBackendLibrariesInput>>,
) -> Result<TemplateLibraryCatalog, TemplateLibraryFixtureError> {
    configure_template_library_catalog(template_library_catalog(db, inputs), settings_cases)
}

pub fn template_library_catalog_with_settings_case_omissions(
    db: &dyn Db,
    inputs: Vec<TemplateLibraryInput>,
    settings_cases: Vec<Vec<TemplateBackendLibrariesInput>>,
) -> Result<TemplateLibraryCatalog, TemplateLibraryFixtureError> {
    configure_template_library_catalog(
        template_library_catalog_with_omissions(db, inputs),
        settings_cases,
    )
}

fn configure_template_library_catalog(
    mut catalog: TemplateLibraryCatalog,
    settings_cases: Vec<Vec<TemplateBackendLibrariesInput>>,
) -> Result<TemplateLibraryCatalog, TemplateLibraryFixtureError> {
    catalog.set_testing_settings_cases(
        settings_cases
            .into_iter()
            .map(|backends| {
                backends
                    .into_iter()
                    .map(|backend| (backend.loadable, backend.builtins))
                    .collect()
            })
            .collect(),
    )?;
    Ok(catalog)
}

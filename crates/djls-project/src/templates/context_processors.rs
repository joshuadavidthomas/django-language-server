use djls_source::File;
use djls_source::Span;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModule;
use crate::python::resolve_prefix;
use crate::settings::TemplateContextProcessorPath;
use crate::settings::django_settings;
use crate::settings::settings_module_file;
use crate::settings::types::PartialTemplateBackend;
use crate::settings::types::SettingCase;
use crate::settings::types::TemplateListEvidence;
use crate::settings::types::WithOrigin;
use crate::settings::types::template_backend_evidence_slots;

/// Context-processor evidence for one backend slot in a feasible settings configuration.
///
/// This remains private extraction evidence. Consumers only enumerate processors known to belong
/// to Django backends through [`TemplateContextProcessors::processors`].
#[derive(Clone, Debug, PartialEq, Eq)]
enum TemplateContextProcessorSlot {
    NotDjango,
    Known(Vec<TemplateContextProcessor>),
    Unknown {
        definite: Vec<TemplateContextProcessor>,
        possible: Vec<TemplateContextProcessor>,
    },
}

impl TemplateContextProcessorSlot {
    fn definite_processors(&self) -> &[TemplateContextProcessor] {
        match self {
            Self::Known(processors)
            | Self::Unknown {
                definite: processors,
                ..
            } => processors,
            Self::NotDjango => &[],
        }
    }
}

/// Backend alternatives belonging to one feasible value of `TEMPLATES`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TemplateContextProcessorConfiguration(Vec<TemplateContextProcessorSlot>);

/// Known context processors discovered across feasible settings alternatives.
///
/// Configuration, backend-slot, and omission evidence is intentionally private. The public
/// contract remains positive enumeration only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateContextProcessors {
    processors: Vec<TemplateContextProcessor>,
    configurations: Vec<TemplateContextProcessorConfiguration>,
    unknown_configurations: bool,
}

impl Default for TemplateContextProcessors {
    fn default() -> Self {
        Self {
            processors: Vec::new(),
            configurations: Vec::new(),
            unknown_configurations: true,
        }
    }
}

impl TemplateContextProcessors {
    #[must_use]
    pub fn processors(&self) -> &[TemplateContextProcessor] {
        &self.processors
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateContextProcessor {
    path: WithOrigin<TemplateContextProcessorPath>,
    module: Option<PythonModule>,
    unresolved_tail: Vec<String>,
}

impl TemplateContextProcessor {
    #[must_use]
    pub fn path_str(&self) -> &str {
        self.path.value().as_str()
    }

    #[must_use]
    pub fn origin(&self) -> (File, Span) {
        let origin = self.path.origin();
        (origin.file, origin.span)
    }

    #[must_use]
    pub fn module(&self) -> Option<&PythonModule> {
        self.module.as_ref()
    }

    #[must_use]
    pub fn unresolved_tail(&self) -> &[String] {
        &self.unresolved_tail
    }
}

#[salsa::tracked(returns(ref))]
pub fn template_context_processors(
    db: &dyn ProjectDb,
    project: Project,
) -> TemplateContextProcessors {
    project.touch_search_path_roots(db);

    if settings_module_file(db, project).is_none() {
        return TemplateContextProcessors::default();
    }

    let settings = django_settings(db, project);
    let mut configurations = Vec::new();
    let mut unknown_configurations = false;

    for case in settings.templates.iter() {
        match case {
            SettingCase::Known(value) => {
                configurations.push(TemplateContextProcessorConfiguration(
                    value
                        .backends
                        .iter()
                        .map(|backend| {
                            if backend.is_django_templates_backend() {
                                TemplateContextProcessorSlot::Known(resolve_processors(
                                    db,
                                    project,
                                    &backend.context_processors,
                                ))
                            } else {
                                TemplateContextProcessorSlot::NotDjango
                            }
                        })
                        .collect(),
                ));
            }
            SettingCase::Dynamic(value) => {
                configurations.push(partial_configuration(
                    db,
                    project,
                    &value.templates.evidence,
                ));
                unknown_configurations |= value
                    .templates
                    .evidence
                    .iter()
                    .any(|evidence| matches!(evidence, TemplateListEvidence::Issue(_)));
            }
            SettingCase::Malformed(value) => {
                configurations.push(partial_configuration(
                    db,
                    project,
                    &value.templates.evidence,
                ));
                unknown_configurations |= value
                    .templates
                    .evidence
                    .iter()
                    .any(|evidence| matches!(evidence, TemplateListEvidence::Issue(_)));
            }
            SettingCase::Unset => {
                configurations.push(TemplateContextProcessorConfiguration(Vec::new()));
            }
        }
    }

    let processors = configurations
        .iter()
        .flat_map(|configuration| &configuration.0)
        .flat_map(TemplateContextProcessorSlot::definite_processors)
        .cloned()
        .collect();

    TemplateContextProcessors {
        processors,
        configurations,
        unknown_configurations,
    }
}

fn partial_configuration(
    db: &dyn ProjectDb,
    project: Project,
    evidence: &[TemplateListEvidence],
) -> TemplateContextProcessorConfiguration {
    TemplateContextProcessorConfiguration(
        template_backend_evidence_slots(evidence)
            .map(|(_backend_index, evidence)| match evidence {
                TemplateListEvidence::Backend(backend) => partial_slot(db, project, backend),
                TemplateListEvidence::Issue(_) => TemplateContextProcessorSlot::Unknown {
                    definite: Vec::new(),
                    possible: Vec::new(),
                },
            })
            .collect(),
    )
}

fn partial_slot(
    db: &dyn ProjectDb,
    project: Project,
    backend: &PartialTemplateBackend,
) -> TemplateContextProcessorSlot {
    let processors = resolve_processors(db, project, &backend.context_processors.known);
    let backend_name = backend
        .backend
        .known
        .as_ref()
        .map(|backend| backend.value.as_str());

    if !backend.backend.issues.is_empty() || backend_name.is_none() {
        return TemplateContextProcessorSlot::Unknown {
            definite: Vec::new(),
            possible: processors,
        };
    }
    if backend_name != Some("django.template.backends.django.DjangoTemplates") {
        return TemplateContextProcessorSlot::NotDjango;
    }
    if backend.options.issues.is_empty() && backend.context_processors.issues.is_empty() {
        TemplateContextProcessorSlot::Known(processors)
    } else {
        TemplateContextProcessorSlot::Unknown {
            definite: processors.clone(),
            possible: processors,
        }
    }
}

fn resolve_processors(
    db: &dyn ProjectDb,
    project: Project,
    paths: &[WithOrigin<TemplateContextProcessorPath>],
) -> Vec<TemplateContextProcessor> {
    paths
        .iter()
        .map(|path| {
            let resolved = resolve_prefix(db, project, path.value.as_str());
            TemplateContextProcessor {
                path: path.clone(),
                module: resolved.module,
                unresolved_tail: resolved.unresolved_tail,
            }
        })
        .collect()
}

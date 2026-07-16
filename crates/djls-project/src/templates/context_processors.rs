use djls_source::File;
use djls_source::Origin;
use djls_source::Span;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModule;
use crate::python::resolve_prefix;
use crate::settings::TemplateContextProcessorPath;
use crate::settings::django_settings;
use crate::settings::settings_module_file;
use crate::settings::types::SettingCase;
use crate::settings::types::TemplateListEvidence;
use crate::settings::types::WithOrigin;

/// Known context processors discovered across feasible settings alternatives.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TemplateContextProcessors {
    processors: Vec<TemplateContextProcessor>,
}

impl TemplateContextProcessors {
    #[must_use]
    pub fn processors(&self) -> &[TemplateContextProcessor] {
        &self.processors
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateContextProcessor {
    path: TemplateContextProcessorPath,
    origin: Origin,
    module: Option<PythonModule>,
    unresolved_tail: Vec<String>,
}

impl TemplateContextProcessor {
    #[must_use]
    pub fn path_str(&self) -> &str {
        self.path.as_str()
    }

    #[must_use]
    pub fn origin(&self) -> (File, Span) {
        (self.origin.file, self.origin.span)
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
    let mut processors = Vec::new();

    for case in settings.templates.iter() {
        match case {
            SettingCase::Known(value) => {
                for backend in &value.backends {
                    if backend.is_django_templates_backend() {
                        processors.extend(resolve_processors(
                            db,
                            project,
                            &backend.context_processors,
                        ));
                    }
                }
            }
            SettingCase::Dynamic(value) => processors.extend(resolve_partial_processors(
                db,
                project,
                &value.templates.evidence,
            )),
            SettingCase::Malformed(value) => processors.extend(resolve_partial_processors(
                db,
                project,
                &value.templates.evidence,
            )),
            SettingCase::Unset => {}
        }
    }

    TemplateContextProcessors { processors }
}

fn resolve_partial_processors(
    db: &dyn ProjectDb,
    project: Project,
    evidence: &[TemplateListEvidence],
) -> Vec<TemplateContextProcessor> {
    let mut processors = Vec::new();
    for evidence in evidence {
        let TemplateListEvidence::Backend(backend) = evidence else {
            continue;
        };
        let backend_name = backend
            .backend
            .known
            .as_ref()
            .map(|backend| backend.value.as_str());
        if backend.backend.issues.is_empty()
            && backend_name == Some("django.template.backends.django.DjangoTemplates")
        {
            processors.extend(resolve_processors(
                db,
                project,
                &backend.context_processors.known,
            ));
        }
    }
    processors
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
                path: path.value().clone(),
                origin: path.origin(),
                module: resolved.module,
                unresolved_tail: resolved.unresolved_tail,
            }
        })
        .collect()
}

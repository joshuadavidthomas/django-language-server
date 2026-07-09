use djls_source::File;
use djls_source::Span;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModule;
use crate::python::resolve_prefix;
use crate::settings::TemplateContextProcessorPath;
use crate::settings::django_settings;
use crate::settings::settings_module_file;
use crate::settings::types::Originated;
use crate::templates::libraries::TemplateInventoryStatus;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateContextProcessors {
    status: TemplateInventoryStatus,
    processors: Vec<TemplateContextProcessor>,
}

impl Default for TemplateContextProcessors {
    fn default() -> Self {
        Self {
            status: TemplateInventoryStatus::NotDiscovered,
            processors: Vec::new(),
        }
    }
}

impl TemplateContextProcessors {
    #[must_use]
    pub fn status(&self) -> TemplateInventoryStatus {
        self.status
    }

    #[must_use]
    pub fn processors(&self) -> &[TemplateContextProcessor] {
        &self.processors
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateContextProcessor {
    path: Originated<TemplateContextProcessorPath>,
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
    let mut status = if settings.templates.is_fully_extracted() {
        TemplateInventoryStatus::Complete
    } else {
        TemplateInventoryStatus::Incomplete
    };
    let mut processors = Vec::new();

    for backend in settings
        .templates
        .backends
        .iter()
        .filter(|backend| backend.is_django_templates_backend())
    {
        if !backend.is_fully_extracted() {
            status = TemplateInventoryStatus::Incomplete;
        }

        for path in &backend.context_processors {
            let resolved = resolve_prefix(db, project, path.value().as_str());
            processors.push(TemplateContextProcessor {
                path: path.clone(),
                module: resolved.module,
                unresolved_tail: resolved.unresolved_tail,
            });
        }
    }

    TemplateContextProcessors { status, processors }
}

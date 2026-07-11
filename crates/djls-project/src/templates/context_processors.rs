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

#[derive(Clone, Debug, PartialEq, Eq)]
enum ContextProcessorOmission {
    Settings,
    Backend,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateContextProcessors(TemplateContextProcessorEvidence);

#[derive(Clone, Debug, PartialEq, Eq)]
enum TemplateContextProcessorEvidence {
    Exhaustive(Vec<TemplateContextProcessor>),
    WithOmissions {
        processors: Vec<TemplateContextProcessor>,
        omissions: Vec<ContextProcessorOmission>,
    },
}

impl Default for TemplateContextProcessors {
    fn default() -> Self {
        Self(TemplateContextProcessorEvidence::WithOmissions {
            processors: Vec::new(),
            omissions: vec![ContextProcessorOmission::Settings],
        })
    }
}

impl TemplateContextProcessors {
    #[must_use]
    pub fn processors(&self) -> &[TemplateContextProcessor] {
        match &self.0 {
            TemplateContextProcessorEvidence::Exhaustive(processors)
            | TemplateContextProcessorEvidence::WithOmissions { processors, .. } => processors,
        }
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
    let mut omissions = if settings.templates.is_fully_extracted() {
        Vec::new()
    } else {
        vec![ContextProcessorOmission::Settings]
    };
    let mut processors = Vec::new();

    for backend in settings
        .templates
        .backends
        .iter()
        .filter(|backend| backend.is_django_templates_backend())
    {
        if !backend.is_fully_extracted() {
            omissions.push(ContextProcessorOmission::Backend);
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

    if omissions.is_empty() {
        TemplateContextProcessors(TemplateContextProcessorEvidence::Exhaustive(processors))
    } else {
        TemplateContextProcessors(TemplateContextProcessorEvidence::WithOmissions {
            processors,
            omissions,
        })
    }
}

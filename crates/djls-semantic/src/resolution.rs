use camino::Utf8PathBuf;
use djls_source::safe_join;
use djls_source::File;
use djls_source::Span;

use crate::db::Db as SemanticDb;
use crate::primitives::InternedTemplateName;
use crate::primitives::Template;
use crate::project::Project;

#[salsa::tracked]
pub(crate) fn discover_templates(db: &dyn SemanticDb, project: Project) -> Vec<Template<'_>> {
    let templates: Vec<_> = project
        .template_files(db)
        .iter()
        .map(|template| {
            Template::new(
                db,
                InternedTemplateName::new(db, template.name().as_str().to_string()),
                template.file(),
            )
        })
        .collect();

    tracing::debug!("Discovered {} total templates", templates.len());
    templates
}

#[salsa::tracked]
pub(crate) fn find_template<'db>(
    db: &'db dyn SemanticDb,
    project: Project,
    template_name: InternedTemplateName<'db>,
) -> Option<Template<'db>> {
    let templates = discover_templates(db, project);

    templates
        .iter()
        .find(|t| t.name(db) == template_name)
        .copied()
}

#[derive(Clone, PartialEq, salsa::Update)]
pub enum ResolveResult<'db> {
    Found(Template<'db>),
    NotFound {
        name: String,
        tried: Vec<Utf8PathBuf>,
    },
}

impl<'db> ResolveResult<'db> {
    #[must_use]
    pub fn ok(self) -> Option<Template<'db>> {
        match self {
            Self::Found(t) => Some(t),
            Self::NotFound { .. } => None,
        }
    }

    #[must_use]
    pub fn is_found(&self) -> bool {
        matches!(self, Self::Found(_))
    }
}

pub fn resolve_template<'db>(db: &'db dyn SemanticDb, name: &str) -> ResolveResult<'db> {
    let template_name = InternedTemplateName::new(db, name.to_string());
    let Some(project) = db.project() else {
        return ResolveResult::NotFound {
            name: name.to_string(),
            tried: Vec::new(),
        };
    };

    if let Some(template) = find_template(db, project, template_name) {
        return ResolveResult::Found(template);
    }

    let tried = project
        .template_dirs(db)
        .as_known()
        .map(|dirs| {
            dirs.iter()
                .filter_map(|dir| safe_join(dir, name).ok())
                .collect()
        })
        .unwrap_or_default();

    ResolveResult::NotFound {
        name: name.to_string(),
        tried,
    }
}

#[salsa::tracked]
pub struct TemplateReference<'db> {
    pub source: Template<'db>,
    pub target: InternedTemplateName<'db>,
    pub span: Span,
}

impl TemplateReference<'_> {
    pub fn source_file(self, db: &dyn SemanticDb) -> File {
        let template = self.source(db);
        template.file(db)
    }
}

pub fn find_references_to_template<'db>(
    db: &'db dyn SemanticDb,
    name: &str,
) -> Vec<TemplateReference<'db>> {
    let Some(project) = db.project() else {
        return Vec::new();
    };

    let template_name = InternedTemplateName::new(db, name.to_string());
    let all_refs = template_reference_index(db, project);

    let matches: Vec<_> = all_refs
        .into_iter()
        .filter(|r| r.target(db) == template_name)
        .collect();

    tracing::debug!(
        "Found {} references to '{}'",
        matches.len(),
        template_name.name(db)
    );
    matches
}

#[salsa::tracked]
fn template_reference_index(db: &dyn SemanticDb, project: Project) -> Vec<TemplateReference<'_>> {
    let mut references = Vec::new();
    let templates = discover_templates(db, project);

    for template in templates {
        for tag in template.tags(db) {
            let tag_name = tag.name();
            if tag_name == "extends" || tag_name == "include" {
                if let Some(template_name) = tag
                    .bits()
                    .first()
                    .and_then(|argument| argument.template_string().quoted_value())
                {
                    references.push(TemplateReference::new(
                        db,
                        template,
                        InternedTemplateName::new(db, template_name.to_string()),
                        tag.span(),
                    ));
                }
            }
        }
    }

    references
}

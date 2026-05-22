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
pub(crate) fn discover_static_templates(
    db: &dyn SemanticDb,
    project: djls_project::Project,
    env: djls_project::DjangoEnvironmentId,
) -> Vec<Template<'_>> {
    djls_project::template_files(db, project, env)
        .templates()
        .iter()
        .map(|template| {
            Template::new(
                db,
                InternedTemplateName::new(db, template.name().to_string()),
                template.file(),
            )
        })
        .collect()
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
    Deferred {
        name: String,
    },
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
            Self::Deferred { .. } | Self::NotFound { .. } => None,
        }
    }

    #[must_use]
    pub fn is_found(&self) -> bool {
        matches!(self, Self::Found(_))
    }
}

pub fn resolve_static_template<'db>(
    db: &'db dyn SemanticDb,
    project: djls_project::Project,
    env: djls_project::DjangoEnvironmentId,
    name: &str,
) -> ResolveResult<'db> {
    let template_name = InternedTemplateName::new(db, name.to_string());
    let inventory = djls_project::template_files(db, project, env.clone());
    let templates = discover_static_templates(db, project, env);
    if let Some(template) = templates
        .iter()
        .find(|template| template.name(db) == template_name)
        .copied()
    {
        return ResolveResult::Found(template);
    }

    if inventory
        .directories()
        .iter()
        .any(|entry| matches!(entry, djls_project::TemplateDirectoryEntry::Deferred { .. }))
    {
        return ResolveResult::Deferred {
            name: name.to_string(),
        };
    }

    ResolveResult::NotFound {
        name: name.to_string(),
        tried: Vec::new(),
    }
}

pub fn resolve_template<'db>(db: &'db dyn SemanticDb, name: &str) -> ResolveResult<'db> {
    let project_facts = djls_project::Db::project(db);
    if let djls_project::DjangoEnvironmentCandidatesOutcome::Ready { candidates, .. } =
        djls_project::django_environment_candidates(db, project_facts)
    {
        if let [candidate] = candidates.as_slice() {
            let result = resolve_static_template(db, project_facts, candidate.id().clone(), name);
            if matches!(
                result,
                ResolveResult::Found(_) | ResolveResult::Deferred { .. }
            ) {
                return result;
            }
        }
    }

    let template_name = InternedTemplateName::new(db, name.to_string());
    let Some(project) = crate::project::Db::project(db) else {
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

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;
    use djls_project::testing::manage_py_path;
    use djls_project::testing::package_init_path;
    use djls_project::testing::project_discovery_set_for_test;
    use djls_project::testing::ready_source_inventory_with_roots_for_test;
    use djls_project::testing::settings_file_path;
    use djls_project::testing::template_path;
    use djls_project::Db as ProjectFactsDb;
    use djls_project::DjangoEnvironmentCandidatesOutcome;
    use djls_project::ProjectDiscovery;

    use super::*;
    use crate::testing::TestDatabase;

    #[test]
    fn static_template_resolution_returns_deferred_for_known_unloaded_directory() {
        let mut db = TestDatabase::new();
        let root = Utf8PathBuf::from("/workspace");
        db.add_file(
            "/workspace/config/settings.py",
            "TEMPLATES = [{'DIRS': ['/workspace/templates']}]\n",
        );
        let project = djls_project::Db::project(&db);
        db.set_project_source_inventory(ready_source_inventory_with_roots_for_test(
            &db,
            vec![root.clone()],
            vec![
                manage_py_path(&root),
                package_init_path(&root, "config"),
                settings_file_path(&root, "config"),
            ],
        ));
        db.set_project_discovery(ProjectDiscovery::Ready(project_discovery_set_for_test(
            &db, root,
        )));

        let result = resolve_template(&db, "emails/welcome.html");

        assert!(matches!(result, ResolveResult::Deferred { .. }));
        assert!(matches!(
            djls_project::django_environment_candidates(&db, project),
            DjangoEnvironmentCandidatesOutcome::Ready { .. }
        ));
    }

    #[test]
    fn static_template_resolution_finds_loaded_configured_template() {
        let mut db = TestDatabase::new();
        let root = Utf8PathBuf::from("/workspace");
        db.add_file(
            "/workspace/config/settings.py",
            "TEMPLATES = [{'DIRS': ['/workspace/templates']}]\n",
        );
        db.add_file("/workspace/templates/emails/welcome.html", "hello");
        let project = djls_project::Db::project(&db);
        db.set_project_source_inventory(ready_source_inventory_with_roots_for_test(
            &db,
            vec![root.clone(), root.join("templates")],
            vec![
                manage_py_path(&root),
                package_init_path(&root, "config"),
                settings_file_path(&root, "config"),
                template_path(&root, "emails/welcome.html"),
            ],
        ));
        db.set_project_discovery(ProjectDiscovery::Ready(project_discovery_set_for_test(
            &db, root,
        )));
        let DjangoEnvironmentCandidatesOutcome::Ready { candidates, .. } =
            djls_project::django_environment_candidates(&db, project)
        else {
            panic!("environment candidates should be ready");
        };

        let result = resolve_static_template(
            &db,
            project,
            candidates[0].id().clone(),
            "emails/welcome.html",
        );
        let public_result = resolve_template(&db, "emails/welcome.html");

        assert!(result.is_found());
        assert!(public_result.is_found());
    }
}

pub fn find_references_to_template<'db>(
    db: &'db dyn SemanticDb,
    name: &str,
) -> Vec<TemplateReference<'db>> {
    let Some(project) = crate::project::Db::project(db) else {
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

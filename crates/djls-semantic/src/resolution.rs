use camino::Utf8PathBuf;
use djls_source::safe_join;
use djls_source::File;
use djls_source::Span;

use crate::db::Db as SemanticDb;
use crate::primitives::InternedTemplateName;
use crate::primitives::Template;
use crate::project::PyModuleName;
use crate::project::TemplateLibraries;
use crate::project::TemplateLibrary;

#[derive(Clone, Debug, Eq, PartialEq, salsa::Update)]
pub enum TemplateLookupIssue {
    Environment(Vec<djls_project::EnvironmentSelectionIssue>),
    Inventory(TemplateInventoryIssue),
    InvalidTemplateName(djls_project::InvalidName),
}

#[derive(Clone, Debug, Eq, PartialEq, salsa::Update)]
pub enum TemplateInventoryIssue {
    Deferred,
    Unavailable,
    Stale,
    UnknownSettingsDir,
}

#[derive(Clone, PartialEq, salsa::Update)]
pub enum TemplateLookupResult<'db> {
    Found(Template<'db>),
    NotFound {
        name: djls_project::TemplateName,
        tried: Vec<Utf8PathBuf>,
    },
    Deferred {
        name: Option<djls_project::TemplateName>,
        issue: TemplateLookupIssue,
    },
}

impl<'db> TemplateLookupResult<'db> {
    #[must_use]
    pub fn ok(self) -> Option<Template<'db>> {
        match self {
            Self::Found(t) => Some(t),
            Self::NotFound { .. } | Self::Deferred { .. } => None,
        }
    }

    #[must_use]
    pub fn is_found(&self) -> bool {
        matches!(self, Self::Found(_))
    }
}

#[salsa::tracked]
pub(crate) fn resolve_static_template(
    db: &dyn SemanticDb,
    project: djls_project::Project,
    env: djls_project::DjangoEnvironmentId,
    name: djls_project::TemplateName,
) -> TemplateLookupResult<'_> {
    let inventory = djls_project::template_files(db, project, env);
    if let Some(template) = inventory
        .templates()
        .iter()
        .find(|template| template.name() == name.as_str())
    {
        return TemplateLookupResult::Found(Template::new(
            db,
            InternedTemplateName::new(db, template.name().to_string()),
            template.file(),
        ));
    }

    if let Some(issue) = inventory_issue(inventory.directories()) {
        return TemplateLookupResult::Deferred {
            name: Some(name),
            issue: TemplateLookupIssue::Inventory(issue),
        };
    }

    let tried = inventory
        .directories()
        .iter()
        .filter_map(|entry| match entry {
            djls_project::TemplateDirectoryEntry::Discovered(directory) => {
                safe_join(directory.path(), name.as_str()).ok()
            }
            _ => None,
        })
        .collect();

    TemplateLookupResult::NotFound { name, tried }
}

pub fn resolve_template<'db>(
    db: &'db dyn SemanticDb,
    source: File,
    name: &str,
) -> TemplateLookupResult<'db> {
    let name = match djls_project::TemplateName::parse(name) {
        Ok(name) => name,
        Err(err) => {
            return TemplateLookupResult::Deferred {
                name: None,
                issue: TemplateLookupIssue::InvalidTemplateName(err),
            };
        }
    };
    let project = djls_project::Db::project(db);
    match djls_project::environment_for_file(db, project, source) {
        djls_project::EnvironmentSelection::Selected(env) => {
            resolve_static_template(db, project, env.clone(), name)
        }
        djls_project::EnvironmentSelection::Unknown { issues }
        | djls_project::EnvironmentSelection::Ambiguous { issues, .. } => {
            TemplateLookupResult::Deferred {
                name: Some(name),
                issue: TemplateLookupIssue::Environment(issues.clone()),
            }
        }
    }
}

pub fn template_libraries_for_file(db: &dyn SemanticDb, source: File) -> Option<TemplateLibraries> {
    let project = djls_project::Db::project(db);
    let env = match djls_project::environment_for_file(db, project, source) {
        djls_project::EnvironmentSelection::Selected(env) => env.clone(),
        djls_project::EnvironmentSelection::Unknown { .. }
        | djls_project::EnvironmentSelection::Ambiguous { .. } => return None,
    };
    let djls_project::SourceFileInventory::Ready(_) = project.source_inventory(db) else {
        return None;
    };
    let inventory = djls_project::loadable_template_libraries(db, project, env);
    let mut libraries = db.template_libraries().clone();

    lower_project_template_libraries(&mut libraries, inventory.libraries());

    Some(libraries)
}

fn lower_project_template_libraries(
    libraries: &mut TemplateLibraries,
    project_libraries: &[djls_project::LoadableTemplateLibrary],
) {
    for library in project_libraries {
        let module = library
            .module()
            .cloned()
            .unwrap_or_else(static_template_library_module);
        let name = library.name().clone();
        libraries
            .loadable
            .entry(name.clone())
            .or_default()
            .push(TemplateLibrary::new_active(name, module, None));
    }
}

fn static_template_library_module() -> PyModuleName {
    PyModuleName::parse("djls_static_template_library")
        .expect("static template library module name should be valid")
}

fn inventory_issue(
    entries: &[djls_project::TemplateDirectoryEntry],
) -> Option<TemplateInventoryIssue> {
    entries.iter().find_map(|entry| match entry {
        djls_project::TemplateDirectoryEntry::Deferred { .. } => {
            Some(TemplateInventoryIssue::Deferred)
        }
        djls_project::TemplateDirectoryEntry::Unavailable { .. } => {
            Some(TemplateInventoryIssue::Unavailable)
        }
        djls_project::TemplateDirectoryEntry::Stale { .. } => Some(TemplateInventoryIssue::Stale),
        djls_project::TemplateDirectoryEntry::UnknownSettingsDir { .. } => {
            Some(TemplateInventoryIssue::UnknownSettingsDir)
        }
        djls_project::TemplateDirectoryEntry::Discovered(_) => None,
    })
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
    use djls_project::manage_py_path;
    use djls_project::package_init_path;
    use djls_project::project_discovery_set_for_test;
    use djls_project::ready_source_inventory_with_roots_for_test;
    use djls_project::settings_file_path;
    use djls_project::template_path;
    use djls_project::Db as ProjectDb;
    use djls_project::DjangoEnvironmentCandidatesOutcome;
    use djls_project::LibraryName;
    use djls_project::ProjectRootDiscovery;
    use salsa::Setter;

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
        db.set_source_file_inventory(ready_source_inventory_with_roots_for_test(
            &db,
            vec![root.clone()],
            vec![
                manage_py_path(&root),
                package_init_path(&root, "config"),
                settings_file_path(&root, "config"),
            ],
        ));
        db.set_project_root_discovery(ProjectRootDiscovery::Ready(project_discovery_set_for_test(
            &db,
            root.clone(),
        )));

        let source = db.create_file(&template_path(&root, "base.html"));
        let result = resolve_template(&db, source, "emails/welcome.html");

        assert!(matches!(result, TemplateLookupResult::Deferred { .. }));
        assert!(matches!(
            djls_project::django_environment_candidates(&db, project),
            DjangoEnvironmentCandidatesOutcome::Ready { .. }
        ));
    }

    #[test]
    fn static_template_inventory_libraries_for_file_include_static_inventory_libraries() {
        let mut db = TestDatabase::new();
        let root = Utf8PathBuf::from("/workspace");
        db.add_file(
            "/workspace/config/settings.py",
            "TEMPLATES = [{'OPTIONS': {'libraries': {'ui': 'blog.templatetags.ui'}}}]\n",
        );
        db.add_file("/workspace/blog/templatetags/ui.py", "");
        let project = djls_project::Db::project(&db);
        db.set_source_file_inventory(ready_source_inventory_with_roots_for_test(
            &db,
            vec![root.clone()],
            vec![
                manage_py_path(&root),
                package_init_path(&root, "config"),
                settings_file_path(&root, "config"),
                root.join("blog/templatetags/ui.py"),
            ],
        ));
        db.set_project_root_discovery(ProjectRootDiscovery::Ready(project_discovery_set_for_test(
            &db,
            root.clone(),
        )));
        let source = db.create_file(&template_path(&root, "base.html"));

        let libraries = template_libraries_for_file(&db, source)
            .expect("selected environment should provide template libraries");

        assert!(libraries
            .loadable
            .contains_key(&LibraryName::parse("ui").expect("test library name should be valid")));
        assert!(matches!(
            djls_project::django_environment_candidates(&db, project),
            DjangoEnvironmentCandidatesOutcome::Ready { .. }
        ));
    }

    #[test]
    fn template_libraries_for_file_merges_runtime_enrichment_hints() {
        let mut db = TestDatabase::new();
        let root = Utf8PathBuf::from("/workspace");
        db.add_file(
            "/workspace/config/settings.py",
            "TEMPLATES = [{'DIRS': ['/workspace/templates']}]\n",
        );
        let project = djls_project::Db::project(&db);
        db.set_source_file_inventory(ready_source_inventory_with_roots_for_test(
            &db,
            vec![root.clone()],
            vec![
                manage_py_path(&root),
                package_init_path(&root, "config"),
                settings_file_path(&root, "config"),
            ],
        ));
        db.set_project_root_discovery(ProjectRootDiscovery::Ready(project_discovery_set_for_test(
            &db,
            root.clone(),
        )));
        project
            .set_enrichment(&mut db)
            .to(djls_project::ProjectEnrichment::Fresh(
                std::collections::BTreeMap::from([(
                    LibraryName::parse("runtime_ui").unwrap(),
                    PyModuleName::parse("blog.templatetags.runtime_ui").unwrap(),
                )]),
            ));
        let source = db.create_file(&template_path(&root, "base.html"));

        let libraries = template_libraries_for_file(&db, source)
            .expect("selected environment should provide template libraries");

        assert!(libraries.loadable.contains_key(
            &LibraryName::parse("runtime_ui").expect("test library name should be valid")
        ));
    }

    #[test]
    fn static_template_inventory_validation_uses_static_inventory() {
        let mut db = TestDatabase::new();
        let root = Utf8PathBuf::from("/workspace");
        db.add_file(
            "/workspace/config/settings.py",
            "TEMPLATES = [{'OPTIONS': {'libraries': {'ui': 'blog.templatetags.ui'}}}]\n",
        );
        db.add_file("/workspace/blog/templatetags/ui.py", "");
        db.add_file("/workspace/templates/base.html", "{% load ui %}");
        db.set_source_file_inventory(ready_source_inventory_with_roots_for_test(
            &db,
            vec![root.clone(), root.join("templates")],
            vec![
                manage_py_path(&root),
                package_init_path(&root, "config"),
                settings_file_path(&root, "config"),
                root.join("blog/templatetags/ui.py"),
                template_path(&root, "base.html"),
            ],
        ));
        db.set_project_root_discovery(ProjectRootDiscovery::Ready(project_discovery_set_for_test(
            &db,
            root.clone(),
        )));
        let file = db.create_file(&template_path(&root, "base.html"));

        crate::validate_template_file(&db, file);
        let errors = crate::validate_template_file::accumulated::<crate::ValidationErrorAccumulator>(
            &db, file,
        );

        assert!(errors.iter().all(|error| !matches!(
            error.0,
            crate::ValidationError::UnknownLibrary { .. }
                | crate::ValidationError::LibraryNotInInstalledApps { .. }
        )));
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
        db.set_source_file_inventory(ready_source_inventory_with_roots_for_test(
            &db,
            vec![root.clone(), root.join("templates")],
            vec![
                manage_py_path(&root),
                package_init_path(&root, "config"),
                settings_file_path(&root, "config"),
                template_path(&root, "emails/welcome.html"),
            ],
        ));
        db.set_project_root_discovery(ProjectRootDiscovery::Ready(project_discovery_set_for_test(
            &db,
            root.clone(),
        )));
        let DjangoEnvironmentCandidatesOutcome::Ready { candidates, .. } =
            djls_project::django_environment_candidates(&db, project)
        else {
            panic!("environment candidates should be ready");
        };

        let source = db.create_file(&template_path(&root, "base.html"));
        let result = resolve_static_template(
            &db,
            project,
            candidates[0].id().clone(),
            djls_project::TemplateName::parse("emails/welcome.html").unwrap(),
        );
        let public_result = resolve_template(&db, source, "emails/welcome.html");

        assert!(result.is_found());
        assert!(public_result.is_found());
    }
}

pub fn find_references_to_template<'db>(
    db: &'db dyn SemanticDb,
    source: File,
    name: &str,
) -> Vec<TemplateReference<'db>> {
    let Ok(name) = djls_project::TemplateName::parse(name) else {
        return Vec::new();
    };
    let project = djls_project::Db::project(db);
    let env = match djls_project::environment_for_file(db, project, source) {
        djls_project::EnvironmentSelection::Selected(env) => env,
        djls_project::EnvironmentSelection::Unknown { .. }
        | djls_project::EnvironmentSelection::Ambiguous { .. } => return Vec::new(),
    };

    let template_name = InternedTemplateName::new(db, name.as_str().to_string());
    let all_refs = static_template_reference_index(db, project, env.clone());

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
fn static_template_reference_index(
    db: &dyn SemanticDb,
    project: djls_project::Project,
    env: djls_project::DjangoEnvironmentId,
) -> Vec<TemplateReference<'_>> {
    let mut references = Vec::new();
    let inventory = djls_project::template_files(db, project, env);

    for project_template in inventory.templates() {
        let template = Template::new(
            db,
            InternedTemplateName::new(db, project_template.name().to_string()),
            project_template.file(),
        );
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

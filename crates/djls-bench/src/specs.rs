use std::collections::BTreeMap;

use camino::Utf8Path;
use djls_conf::DjangoVersion;
use djls_conf::TagSpecDef;
use djls_ide::prime_template_library_products;
use djls_project::InvalidModuleName;
use djls_project::Project;
use djls_project::PythonEnvironment;
use djls_project::PythonModuleName;
use djls_project::SearchPaths;
use djls_semantic::bundled_django_tag_specs;
use djls_source::Db as SourceDb;
use djls_source::FileError;
use djls_testing::ProjectSettings;

use crate::Db;

#[derive(Clone, Debug, thiserror::Error)]
pub enum BenchmarkSetupError {
    #[error("invalid benchmark Python module name {value:?}: {source}")]
    InvalidModuleName {
        value: &'static str,
        #[source]
        source: InvalidModuleName,
    },
    #[error("failed to register a benchmark source: {0}")]
    RegisterSource(#[from] FileError),
    #[error("the realistic benchmark fixture did not install a Project")]
    MissingProject,
}

fn install_template_library_fixture(db: &mut Db) -> Result<(), BenchmarkSetupError> {
    db.add_fixture_source("/project/__init__.py", "");
    db.add_fixture_source(
        "/project/settings.py",
        ProjectSettings {
            libraries: BTreeMap::from([
                ("i18n".to_string(), "django.templatetags.i18n".to_string()),
                (
                    "static".to_string(),
                    "django.templatetags.static".to_string(),
                ),
            ]),
            ..ProjectSettings::default()
        }
        .settings_py(),
    );

    // Django comes from a bundled release, as Django Discovery mounts it when Django is not
    // installed. The newest line matches the Django corpus templates checked by benchmarks.
    let root = Utf8Path::new("/");
    let python_environment = PythonEnvironment::Auto;
    let django_version = DjangoVersion::Django61;
    let mut search_paths =
        SearchPaths::from_project_settings(db.file_system(), root, &python_environment, &[]);
    search_paths.add_bundled_django(db.file_system(), root, Some(django_version));
    search_paths.register_roots(db);
    let settings_module = PythonModuleName::parse("project.settings").map_err(|source| {
        BenchmarkSetupError::InvalidModuleName {
            value: "project.settings",
            source,
        }
    })?;
    let project = Project::new(
        db,
        root.to_path_buf(),
        search_paths,
        python_environment,
        Some(settings_module),
        Some(django_version),
        Vec::new(),
        Vec::new(),
        TagSpecDef::default(),
    );
    db.set_project(project);
    Ok(())
}

/// Create a benchmark `Db` configured for semantic structure projections.
pub fn structure_db() -> Result<Db, BenchmarkSetupError> {
    Ok(Db::new().with_projectless_tag_specs(bundled_django_tag_specs().clone()))
}

/// Create a benchmark `Db` with a Project whose Django Template Libraries come
/// from the bundled Django sources.
pub fn realistic_db() -> Result<Db, BenchmarkSetupError> {
    configure_realistic_db(Db::new())
}

fn configure_realistic_db(mut db: Db) -> Result<Db, BenchmarkSetupError> {
    install_template_library_fixture(&mut db)?;
    Ok(db)
}

/// Create the realistic database with production intrinsic priming complete.
pub fn primed_realistic_db() -> Result<Db, BenchmarkSetupError> {
    let db = realistic_db()?;
    prime_template_library_products(&db).ok_or(BenchmarkSetupError::MissingProject)?;
    Ok(db)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use djls_project::Db as ProjectDb;
    use djls_project::ScopedTemplateLibraries;
    use djls_project::TagRole;
    use djls_project::TemplateLibrary;
    use djls_project::template_library_catalog;
    use djls_semantic::SemanticOffsetContext;
    use djls_semantic::tag_spec_at;
    use djls_source::Offset;
    use djls_templates::parse_template;
    use salsa::Event;

    use super::Db;
    use super::configure_realistic_db;
    use super::primed_realistic_db;

    impl Db {
        pub(crate) fn realistic_with_event_log(events: Arc<Mutex<Vec<Event>>>) -> Self {
            configure_realistic_db(Self::with_event_log(events))
                .expect("realistic benchmark database should initialize")
        }
    }

    #[test]
    fn realistic_project_fuses_canonical_builtins_and_converges_loads() {
        let mut db =
            primed_realistic_db().expect("primed realistic benchmark database should initialize");
        let project = db
            .project()
            .expect("realistic fixture should install a Project");
        let environment =
            ScopedTemplateLibraries::from_project_inventory(template_library_catalog(&db, project));
        let builtin_modules: Vec<_> = environment
            .resolved_libraries()
            .into_iter()
            .filter(|library| library.load_name().is_none())
            .map(TemplateLibrary::module_name_str)
            .collect();
        assert_eq!(
            builtin_modules,
            [
                "django.template.defaulttags",
                "django.template.defaultfilters",
                "django.template.loader_tags",
            ]
        );

        let source = "{% load i18n %}{% trans \"hello\" %}";
        let file = db
            .file_with_contents("/templates/semantic-contract.html", source)
            .expect("semantic contract fixture should register");
        let nodelist = parse_template(&db, file)
            .expect("semantic contract fixture should be a readable Template");
        let load_position = source
            .find("load")
            .expect("semantic contract source should contain the load tag");
        let load_position = u32::try_from(load_position)
            .expect("semantic contract load position should fit in u32");
        assert_eq!(
            tag_spec_at(&db, file, nodelist, load_position, "load").and_then(|spec| spec.role()),
            Some(TagRole::TemplateLibraryLoader),
        );

        let trans_position = source
            .find("trans")
            .expect("semantic contract source should contain the trans tag");
        let trans_position = u32::try_from(trans_position)
            .expect("semantic contract trans position should fit in u32");
        assert!(matches!(
            SemanticOffsetContext::from_offset(&db, file, Offset::new(trans_position)),
            SemanticOffsetContext::Tag {
                name,
                loaded_libraries,
                ..
            } if name == "trans" && loaded_libraries == ["i18n"]
        ));
    }
}

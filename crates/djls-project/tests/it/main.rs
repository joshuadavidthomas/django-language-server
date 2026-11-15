// Integration tests share one binary so they link the dependency graph once.
use camino::Utf8Path;
use djls_conf::Settings;
use djls_project::EnvironmentAssemblyError;
use djls_project::Project;
use djls_project::run_django_discovery;
use djls_testing::ProjectFixtureDatabase;

mod bundled_django;
mod corpus_settings_redaction;
mod django_projects;
mod django_version;
mod extraction;
mod model_relations;
mod project_performance;
mod resolve;
mod settings;
mod settings_extraction;
mod tag_regex;
mod template_libraries;
mod templates;

fn initialize_project<Db: ProjectFixtureDatabase>(
    db: &mut Db,
    root: &Utf8Path,
    settings: &Settings,
) -> Result<Project, EnvironmentAssemblyError> {
    let project = Project::initial(db, root, settings);
    db.set_fixture_project(project);
    project.reload_from_settings(db, settings);
    run_django_discovery(db)?;
    Ok(project)
}

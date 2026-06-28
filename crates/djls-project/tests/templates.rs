use djls_project::*;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;

fn project_with_templates(
    db: &mut TestDatabase,
    template_dirs: Vec<&str>,
    templates: Vec<(&str, &str, &str)>,
) -> Project {
    let dirs_literal = template_dirs
        .into_iter()
        .map(|dir| format!("'{dir}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let settings_source = format!(
        "INSTALLED_APPS = []\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [{dirs_literal}], 'APP_DIRS': False}}]\n"
    );
    let fixture = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings_source);
    templates
        .into_iter()
        .fold(fixture, |fixture, (_name, path, source)| {
            fixture.file(path, source)
        })
        .build(db)
}

#[test]
fn template_origins_preserve_django_search_order() {
    let mut db = TestDatabase::new();
    let project = project_with_templates(
        &mut db,
        vec!["/test/project/templates", "/test/project/app/templates"],
        vec![
            (
                "base.html",
                "/test/project/templates/base.html",
                "project base",
            ),
            (
                "base.html",
                "/test/project/app/templates/base.html",
                "app base",
            ),
            (
                "account/detail.html",
                "/test/project/app/templates/account/detail.html",
                "detail",
            ),
        ],
    );

    let names: Vec<_> = template_resolution(&db, project)
        .origins(&db)
        .map(|origin| origin.template_name(&db).name(&db).clone())
        .collect();

    assert_eq!(names, ["base.html", "account/detail.html", "base.html"]);
}

#[test]
fn derived_template_origins_keep_shadowed_names_in_template_dir_order() {
    let mut db = TestDatabase::new();
    let project = project_with_templates(
        &mut db,
        vec!["/test/project/templates", "/test/project/app/templates"],
        vec![
            (
                "shared.html",
                "/test/project/app/templates/shared.html",
                "app shared",
            ),
            (
                "shared.html",
                "/test/project/templates/shared.html",
                "project shared",
            ),
        ],
    );

    let paths: Vec<_> = template_resolution(&db, project)
        .origins(&db)
        .map(|origin| origin.file(&db).path(&db).as_str())
        .collect();

    assert_eq!(
        paths,
        [
            "/test/project/templates/shared.html",
            "/test/project/app/templates/shared.html",
        ]
    );
}

#[test]
fn find_template_returns_first_origin_for_duplicate_template_names() {
    let mut db = TestDatabase::new();
    let project = project_with_templates(
        &mut db,
        vec!["/test/project/templates", "/test/project/app/templates"],
        vec![
            (
                "base.html",
                "/test/project/templates/base.html",
                "project base",
            ),
            (
                "base.html",
                "/test/project/app/templates/base.html",
                "app base",
            ),
        ],
    );

    let name = TemplateName::new(&db, "base.html".to_string());
    let result = template_resolution(&db, project).resolve(&db, name);
    let FindTemplateResult::Found(origin) = result else {
        panic!("expected base.html to resolve");
    };

    assert_eq!(
        origin.file(&db).path(&db).as_str(),
        "/test/project/templates/base.html"
    );
}

#[test]
fn find_template_reports_tried_sources_for_missing_template() {
    let mut db = TestDatabase::new();
    let project = project_with_templates(
        &mut db,
        vec!["/test/project/templates", "/test/project/app/templates"],
        Vec::new(),
    );

    let name = TemplateName::new(&db, "missing.html".to_string());
    let result = template_resolution(&db, project).resolve(&db, name);
    let FindTemplateResult::DoesNotExist(error) = result else {
        panic!("expected missing.html to be missing");
    };
    let tried: Vec<_> = error
        .tried
        .iter()
        .map(|source| source.path.as_str())
        .collect();

    assert_eq!(
        tried,
        [
            "/test/project/templates/missing.html",
            "/test/project/app/templates/missing.html"
        ]
    );
}

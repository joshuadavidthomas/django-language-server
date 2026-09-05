use djls_ide::prepare_project_template_analysis;
use djls_ide::warm_cache_phases;
use djls_project::Db as _;
use djls_project::run_django_discovery;
use djls_project::template_resolution;
use djls_testing::ProjectFixture;
use djls_testing::SalsaEventLog;
use djls_testing::TestDatabase;

#[test]
fn discovery_and_warmup_defer_model_work() {
    let events = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(events.clone());
    ProjectFixture::new("/project")
        .django_settings_module("settings")
        .file(
            "/project/settings.py",
            "INSTALLED_APPS = ['app']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/project/templates'], 'OPTIONS': {'builtins': ['app.templatetags.tags']}}]\n",
        )
        .file("/project/app/__init__.py", "")
        .file(
            "/project/app/models.py",
            "from django.db import models\nclass Article(models.Model):\n    pass\n",
        )
        .file("/project/app/templatetags/__init__.py", "")
        .file(
            "/project/app/templatetags/tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef article_title(): pass\n",
        )
        .file(
            "/project/templates/article.html",
            "{% article_title %}",
        )
        .install(&mut db)
        .expect("warm-up fixture should install");
    events
        .take()
        .expect("fixture setup Salsa events should be cleared");

    let facts = run_django_discovery(&mut db)
        .expect("Django Discovery should assemble its phases")
        .expect("fixture should have a Project");
    assert!(facts.file_paths().contains(&"/project/settings.py".into()));
    assert!(
        facts
            .file_paths()
            .contains(&"/project/app/templatetags/tags.py".into())
    );
    assert!(
        !facts
            .file_paths()
            .contains(&"/project/app/models.py".into()),
        "routine Django Discovery should not index Django Model sources",
    );

    prepare_project_template_analysis(&db).expect("Template analysis should prepare");
    for phase in warm_cache_phases() {
        assert!(phase.run(&db).count().is_some_and(|count| count > 0));
    }

    let project = db.project().expect("fixture should have a Project");
    assert_eq!(template_resolution(&db, project).origins(&db).count(), 1);

    let names = events
        .take_will_execute_names(&db)
        .expect("discovery and warm-up Salsa events should be readable");
    for useful in [
        "settings_sources",
        "template_library_catalog",
        "semantic_grammar_vocabulary",
        "template_directory_index",
    ] {
        assert!(
            names
                .iter()
                .any(|name| name.rsplit("::").next() == Some(useful)),
            "discovery and warm-up did not run {useful}",
        );
    }
    for deferred in ["model_modules", "compute_model_graph", "extract_models"] {
        assert!(
            !names
                .iter()
                .any(|name| name.rsplit("::").next() == Some(deferred)),
            "routine discovery and warm-up ran {deferred}",
        );
    }
}

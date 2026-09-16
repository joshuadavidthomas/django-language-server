use camino::Utf8Path;
use djls_ide::prepare_project_template_analysis;
use djls_ide::prime_template_library_products;
use djls_ide::warm_cache_phases;
use djls_project::Db as _;
use djls_project::run_django_discovery;
use djls_project::template_resolution;
use djls_source::ChangeEvent;
use djls_source::SourceChanges;
use djls_testing::ProjectFixture;
use djls_testing::SalsaEventLog;
use djls_testing::TestDatabase;
use djls_testing::execution_count;

fn install_project_fixture(db: &mut TestDatabase) -> Result<(), Box<dyn std::error::Error>> {
    ProjectFixture::new("/project")
        .django_settings_module("settings")
        .file(
            "/project/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/project/templates'], 'OPTIONS': {'builtins': ['tags']}}]\n",
        )
        .file(
            "/project/tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef hello(): pass\n@register.filter\ndef shout(value): pass\n",
        )
        .file("/project/templates/page.html", "{% hello %}{{ value|shout }}")
        .install(db)?;
    Ok(())
}

#[test]
fn final_state_matrix_01_04_shared_prime_is_exact_and_has_no_template_work() {
    let events = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(events.clone());
    install_project_fixture(&mut db).expect("warmup project fixture should install");
    events
        .take()
        .expect("initial warmup Salsa events should be cleared");

    let primed = prime_template_library_products(&db).expect("fixture has a Project");
    let covered_paths: Vec<_> = primed
        .covered_files()
        .map(|file| file.path(&db).as_str())
        .collect();
    assert!(covered_paths.contains(&"/project/tags.py"));
    assert!(covered_paths.contains(&"/project/settings.py"));
    assert_eq!(
        covered_paths.len(),
        primed.reprime_files().len() + primed.full_reload_files().len()
    );
    assert!(
        primed
            .reprime_files()
            .iter()
            .any(|file| file.path(&db) == Utf8Path::new("/project/tags.py"))
    );
    assert_eq!(primed.full_reload_files().len(), 1);

    let names = events
        .take_will_execute_names(&db)
        .expect("warmup Salsa events should be read");
    assert_eq!(
        execution_count(&names, "template_library_structure_facts"),
        primed.library_count()
    );
    assert_eq!(execution_count(&names, "semantic_grammar_vocabulary"), 1);
    for forbidden in [
        "template_library_tag_rule_analysis",
        "template_library_filter_facts",
        "library_tag_specs",
        "library_filter_specs",
        "parse_template",
        "template_analysis_projection_for_file_in_scope",
        "validate_template_file",
    ] {
        assert_eq!(
            execution_count(&names, forbidden),
            0,
            "priming ran {forbidden}"
        );
    }

    let repeated = prime_template_library_products(&db).expect("fixture has a Project");
    assert_eq!(repeated, primed);
    let names = events
        .take_will_execute_names(&db)
        .expect("repeated warmup Salsa events should be read");
    for intrinsic in [
        "template_library_definition_facts",
        "template_library_structure_facts",
        "template_library_inventory_dependencies",
        "library_tag_structure_specs",
        "library_tag_specs",
        "library_filter_specs",
        "semantic_grammar_vocabulary",
    ] {
        assert_eq!(
            execution_count(&names, intrinsic),
            0,
            "repeated prime ran {intrinsic}"
        );
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "keep multiline Python fixtures inline"
)]
fn priming_many_custom_libraries_and_rule_helper_edits_keep_details_cold() {
    let events = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(events.clone());
    ProjectFixture::new("/project")
        .django_settings_module("settings")
        .file(
            "/project/settings.py",
            r"INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {'alpha': 'alpha_tags', 'beta': 'beta_tags', 'gamma': 'gamma_tags'}, 'builtins': []}}]
",
        )
        .file(
            "/project/alpha_tags.py",
            r"from django import template
from helper import bits
register = template.Library()
@register.tag(name='alpha')
def alpha(parser, token):
    parts = bits(token)
    if len(parts) != 2: raise template.TemplateSyntaxError('count')
    return template.Node()
@register.filter
def alpha_filter(value): pass
",
        )
        .file(
            "/project/beta_tags.py",
            r"from django import template
register = template.Library()
@register.simple_tag
def beta(value): pass
@register.filter
def beta_filter(value): pass
",
        )
        .file(
            "/project/gamma_tags.py",
            r"from django import template
register = template.Library()
@register.simple_tag
def gamma(value): pass
@register.filter
def gamma_filter(value): pass
",
        )
        .file(
            "/project/helper.py",
            r"def bits(token):
    return token.split_contents()[1:]
",
        )
        .install(&mut db)
        .expect("multi-library warmup fixture should install");
    events.take().expect("fixture setup events should clear");

    let primed = prime_template_library_products(&db).expect("fixture has a Project");
    assert_eq!(primed.library_count(), 6);
    assert!(
        primed
            .covered_files()
            .all(|file| file.path(&db) != Utf8Path::new("/project/helper.py")),
        "a dependency used only for Tag Rule inference must not be eager coverage"
    );
    let names = events
        .take_will_execute_names(&db)
        .expect("first prime events should be readable");
    assert_eq!(
        execution_count(&names, "template_library_structure_facts"),
        primed.library_count(),
        "{names:?}"
    );
    for detail in [
        "template_library_tag_rule_analysis",
        "template_library_filter_facts",
        "library_tag_specs",
        "library_filter_specs",
    ] {
        assert_eq!(
            execution_count(&names, detail),
            0,
            "prime executed {detail}"
        );
    }

    db.add_file(
        "/project/helper.py",
        r"def bits(token):
    return token.split_contents()[2:]
",
    )
    .expect("rule helper should change");
    SourceChanges::new([ChangeEvent::ContentChanged("/project/helper.py".into())]).apply(&mut db);
    let repeated = prime_template_library_products(&db).expect("fixture has a Project");
    assert_eq!(repeated, primed);
    let names = events
        .take_will_execute_names(&db)
        .expect("helper-edit prime events should be readable");
    for detail in [
        "template_library_tag_rule_analysis",
        "template_library_filter_facts",
        "library_tag_specs",
        "library_filter_specs",
    ] {
        assert_eq!(
            execution_count(&names, detail),
            0,
            "rule-helper-only edit caused prime to execute {detail}"
        );
    }
}

#[test]
fn priming_covers_imported_registration_sources() {
    let mut db = TestDatabase::new();
    ProjectFixture::new("/project")
        .django_settings_module("settings")
        .file(
            "/project/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'builtins': ['app.templatetags.tags']}}]\n",
        )
        .file("/project/app/__init__.py", "")
        .file("/project/app/templatetags/__init__.py", "")
        .file(
            "/project/app/templatetags/tags.py",
            "from django import template\nfrom . import implementation\nregister = template.Library()\nregister.tag(implementation.TAG, implementation.compile_tag)\n",
        )
        .file(
            "/project/app/templatetags/implementation.py",
            "TAG = 'imported'\ndef compile_tag(parser, token): pass\n",
        )
        .install(&mut db)
        .expect("imported registration warmup fixture should install");

    let primed = prime_template_library_products(&db).expect("fixture has a Project");
    for path in [
        "/project/app/__init__.py",
        "/project/app/templatetags/__init__.py",
        "/project/app/templatetags/implementation.py",
    ] {
        assert!(
            primed
                .reprime_files()
                .iter()
                .any(|file| file.path(&db) == Utf8Path::new(path)),
            "priming coverage should include {path}",
        );
    }
}

#[test]
fn project_template_preparation_orders_and_reuses_shared_products() {
    let events = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(events.clone());
    install_project_fixture(&mut db).expect("warmup project fixture should install");
    events
        .take()
        .expect("initial warmup Salsa events should be cleared");

    prepare_project_template_analysis(&db).expect("fixture has a Project");
    let project = db.project().expect("fixture has a Project");
    assert_eq!(template_resolution(&db, project).origins(&db).count(), 1);

    let names = events
        .take_will_execute_names(&db)
        .expect("preparation Salsa events should be read");
    let intrinsic_position = names
        .iter()
        .position(|name| name.rsplit("::").next() == Some("semantic_grammar_vocabulary"))
        .expect("preparation primes intrinsic products");
    let index_position = names
        .iter()
        .position(|name| name.rsplit("::").next() == Some("template_directory_index"))
        .expect("preparation builds the shared Template index");
    assert!(intrinsic_position < index_position);

    assert_eq!(prepare_project_template_analysis(&db), Some(()));
    let repeated_names = events
        .take_will_execute_names(&db)
        .expect("repeated preparation Salsa events should be read");
    for shared_query in ["semantic_grammar_vocabulary", "template_directory_index"] {
        assert_eq!(
            execution_count(&repeated_names, shared_query),
            0,
            "repeated preparation ran {shared_query}"
        );
    }
}

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

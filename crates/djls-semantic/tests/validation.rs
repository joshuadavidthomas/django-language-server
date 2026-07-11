use std::collections::BTreeMap;
use std::fmt::Write;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_semantic::Db as SemanticDb;
use djls_semantic::ValidationError;
use djls_semantic::ValidationErrorAccumulator;
use djls_semantic::compute_tag_specs;
use djls_semantic::validate_template_file;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;
use djls_testing::collect_errors;
use djls_testing::partial_validation_db;
use djls_testing::standard_validation_db;

fn standard_db() -> TestDatabase {
    standard_validation_db()
}

fn partial_db() -> TestDatabase {
    partial_validation_db()
}

fn partial_ambiguous_db() -> TestDatabase {
    let db = partial_db();
    db.add_file(
        "/example/alpha/templatetags/alpha.py",
        "from django import template\nregister = template.Library()\n@register.tag(name='shared')\ndef shared_tag(parser, token): pass\n@register.filter(name='shared')\ndef shared_filter(value): pass\n",
    );
    db.add_file(
        "/example/beta/templatetags/beta.py",
        "from django import template\nregister = template.Library()\n@register.tag(name='shared')\ndef shared_tag(parser, token): pass\n@register.filter(name='shared')\ndef shared_filter(value): pass\n",
    );
    db
}

fn collect_all_errors(db: &TestDatabase, source: &str) -> Vec<ValidationError> {
    collect_errors(db, "test.html", source)
}

fn collect_file_errors(db: &TestDatabase, path: &str) -> Vec<ValidationError> {
    let file = db.file(Utf8Path::new(path));
    validate_template_file(db, file);
    validate_template_file::accumulated::<ValidationErrorAccumulator>(db, file)
        .into_iter()
        .map(|error| error.0.clone())
        .collect()
}

#[test]
fn open_backend_after_concrete_membership_keeps_validation_inconclusive() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [UNKNOWN, '/proj/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alpha_tags'}}},\n    UNKNOWN,\n]\n";
    ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/settings.py", settings)
        .file(
            "/proj/alpha_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag(name='shared_tag')\ndef alpha(value):\n    pass\n",
        )
        .file(
            "/proj/shared/page.html",
            "{% load shared %}{% shared_tag %}",
        )
        .install(&mut db);

    let errors = collect_file_errors(&db, "/proj/shared/page.html");
    assert!(
        !errors
            .iter()
            .any(|error| matches!(error, ValidationError::ExtractedRuleViolation { .. })),
        "an additional open backend must prevent a backend-local rule from becoming definite: {errors:?}"
    );
}

#[test]
fn wholly_unknown_templates_branch_keeps_file_validation_inconclusive() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nif FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alpha_tags'}}}]\nelse:\n    TEMPLATES = UNKNOWN\n";
    ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/settings.py", settings)
        .file(
            "/proj/alpha_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef shared_tag(value):\n    pass\n",
        )
        .file(
            "/proj/templates/page.html",
            "{% load shared %}{% shared_tag %}",
        )
        .install(&mut db);

    let errors = collect_file_errors(&db, "/proj/templates/page.html");
    assert!(
        !errors
            .iter()
            .any(|error| matches!(error, ValidationError::ExtractedRuleViolation { .. })),
        "the wholly unknown settings branch must preserve spec uncertainty: {errors:?}"
    );
}

#[test]
fn unknown_configured_alias_keys_suppress_installed_app_guidance() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {**UNKNOWN}}}]\n";
    ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/settings.py", settings)
        .file("/proj/crispy/__init__.py", "")
        .file("/proj/crispy/templatetags/__init__.py", "")
        .file(
            "/proj/crispy/templatetags/shared.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef crispy_tag():\n    pass\n@register.filter\ndef crispy_filter(value):\n    return value\n",
        )
        .file(
            "/proj/templates/page.html",
            "{% crispy_tag %}{{ value|crispy_filter }}{% load shared %}",
        )
        .install(&mut db);

    let errors = collect_file_errors(&db, "/proj/templates/page.html");
    assert!(
        !errors.iter().any(|error| matches!(
            error,
            ValidationError::TagNotInInstalledApps { .. }
                | ValidationError::FilterNotInInstalledApps { .. }
                | ValidationError::LibraryNotInInstalledApps { .. }
        )),
        "dynamic alias keys must suppress definitive installed-app guidance: {errors:?}"
    );
}

#[test]
fn dynamic_installed_apps_suppress_guidance_without_template_backends() {
    for templates in ["TEMPLATES = []\n", ""] {
        let mut db = TestDatabase::new();
        let settings = format!("INSTALLED_APPS = [UNKNOWN]\n{templates}");
        ProjectFixture::new("/proj")
            .django_settings_module("myproject.settings")
            .file("/proj/myproject/settings.py", settings)
            .file("/proj/crispy/__init__.py", "")
            .file("/proj/crispy/templatetags/__init__.py", "")
            .file(
                "/proj/crispy/templatetags/crispy.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef crispy_tag(): pass\n@register.filter\ndef crispy_filter(value): return value\n",
            )
            .file(
                "/proj/page.html",
                "{% crispy_tag %}{{ value|crispy_filter }}{% load crispy %}",
            )
            .install(&mut db);

        let errors = collect_file_errors(&db, "/proj/page.html");
        assert!(
            !errors.iter().any(|error| matches!(
                error,
                ValidationError::TagNotInInstalledApps { .. }
                    | ValidationError::FilterNotInInstalledApps { .. }
                    | ValidationError::LibraryNotInInstalledApps { .. }
            )),
            "dynamic apps with {templates:?} must suppress definitive guidance: {errors:?}"
        );
    }
}

#[test]
fn partial_django_backend_keeps_configured_library_validation_inconclusive() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'custom_tags'}}, unknown_key: 'maybe'}]\n";
    ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/settings.py", settings)
        .file(
            "/proj/custom_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef configured(value):\n    pass\n",
        )
        .file(
            "/proj/templates/page.html",
            "{% load custom %}{% configured %}",
        )
        .install(&mut db);

    let errors = collect_file_errors(&db, "/proj/templates/page.html");
    assert!(
        !errors
            .iter()
            .any(|error| matches!(error, ValidationError::ExtractedRuleViolation { .. })),
        "an overriding backend-key issue must keep configured rules inconclusive: {errors:?}"
    );
}

#[test]
fn validation_uses_only_the_library_environment_of_the_resolving_backend() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/a'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alpha_tags'}}},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/b'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'beta_tags'}}},\n]\n";
    ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/settings.py", settings)
        .file(
            "/proj/alpha_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef alpha():\n    pass\n",
        )
        .file(
            "/proj/beta_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef beta():\n    pass\n",
        )
        .file("/proj/a/alpha.html", "{% load shared %}{% alpha %}{% beta %}")
        .file("/proj/b/beta.html", "{% load shared %}{% alpha %}{% beta %}")
        .install(&mut db);

    let alpha_errors = collect_file_errors(&db, "/proj/a/alpha.html");
    let beta_errors = collect_file_errors(&db, "/proj/b/beta.html");

    assert!(!alpha_errors.iter().any(|error| matches!(
        error,
        ValidationError::UnknownTag { tag, .. }
            | ValidationError::UnloadedTag { tag, .. } if tag == "alpha"
    )));
    assert!(!beta_errors.iter().any(|error| matches!(
        error,
        ValidationError::UnknownTag { tag, .. }
            | ValidationError::UnloadedTag { tag, .. } if tag == "beta"
    )));
}

#[test]
fn conflicting_backend_specs_do_not_produce_argument_arity_or_structure_diagnostics() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nif FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alpha_tags'}}}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'beta_tags'}}}]\n";
    ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/settings.py", settings)
        .file(
            "/proj/alpha_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag(name='shared_tag')\ndef alpha(value):\n    pass\n@register.filter(name='shared_filter')\ndef alpha_filter(value, arg):\n    return value\n@register.tag(name='panel')\ndef alpha_panel(parser, token):\n    body = parser.parse(('endalpha',))\n    return Node(body)\n",
        )
        .file(
            "/proj/beta_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag(name='shared_tag')\ndef beta():\n    pass\n@register.filter(name='shared_filter')\ndef beta_filter(value):\n    return value\n@register.tag(name='panel')\ndef beta_panel(parser, token):\n    body = parser.parse(('endbeta',))\n    return Node(body)\n",
        )
        .file(
            "/proj/shared/page.html",
            "{% load shared %}{% shared_tag %}{{ value|shared_filter }}{% panel %}",
        )
        .install(&mut db);

    let errors = collect_file_errors(&db, "/proj/shared/page.html");
    assert!(
        !errors.iter().any(|error| matches!(
            error,
            ValidationError::ExtractedRuleViolation { .. }
                | ValidationError::FilterMissingArgument { .. }
                | ValidationError::FilterUnexpectedArgument { .. }
        )),
        "conflicting signatures must remain inconclusive: {errors:?}"
    );
    assert!(
        !errors.iter().any(
            |error| matches!(error, ValidationError::UnclosedTag { tag, .. } if tag == "panel")
        ),
        "conflicting block shapes must remain inconclusive: {errors:?}"
    );
}

#[test]
fn unloaded_custom_collision_does_not_override_builtin_if_grammar() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'collisions': 'collision_tags'}}}]\n";
    ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/settings.py", settings)
        .file(
            "/proj/collision_tags.py",
            "from django import template\nregister = template.Library()\n@register.tag(name='if')\ndef custom_if(parser, token):\n    body = parser.parse(('endcustom',))\n    return Node(body)\n",
        )
        .file(
            "/proj/templates/page.html",
            "{% if condition %}yes{% endif %}",
        )
        .file("/proj/templates/unclosed.html", "{% if condition %}")
        .install(&mut db);

    let errors = collect_file_errors(&db, "/proj/templates/page.html");
    assert!(
        !errors.iter().any(|error| matches!(
            error,
            ValidationError::UnknownTag { tag, .. }
                | ValidationError::UnloadedTag { tag, .. }
                | ValidationError::UnclosedTag { tag, .. }
                | ValidationError::OrphanedTag { tag, .. } if tag == "if" || tag == "endif"
        )),
        "an unloaded collision must not replace builtin if structure: {errors:?}"
    );

    let unclosed_errors = collect_file_errors(&db, "/proj/templates/unclosed.html");
    assert!(
        unclosed_errors
            .iter()
            .any(|error| matches!(error, ValidationError::UnclosedTag { tag, .. } if tag == "if")),
        "the fixture must expose the builtin if block specification: {unclosed_errors:?}"
    );
}

#[test]
fn project_fixture_registers_builtin_for_structure() {
    let mut db = TestDatabase::new();
    ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file(
            "/proj/myproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False}]\n",
        )
        .file(
            "/proj/templates/valid.html",
            "{% for item in items %}{{ item }}{% empty %}empty{% endfor %}",
        )
        .file("/proj/templates/unclosed.html", "{% for item in items %}")
        .install(&mut db);

    let valid_errors = collect_file_errors(&db, "/proj/templates/valid.html");
    assert!(
        !valid_errors.iter().any(|error| matches!(
            error,
            ValidationError::UnknownTag { tag, .. }
                | ValidationError::UnloadedTag { tag, .. }
                | ValidationError::UnclosedTag { tag, .. }
                | ValidationError::OrphanedTag { tag, .. }
                if tag == "for" || tag == "empty" || tag == "endfor"
        )),
        "the fixture must recognize the complete builtin for structure: {valid_errors:?}"
    );

    let unclosed_errors = collect_file_errors(&db, "/proj/templates/unclosed.html");
    assert!(
        unclosed_errors
            .iter()
            .any(|error| matches!(error, ValidationError::UnclosedTag { tag, .. } if tag == "for")),
        "the fixture must expose the builtin for block specification: {unclosed_errors:?}"
    );
}

#[test]
fn loaded_library_contract_wins_over_conflicting_unloaded_library() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'alpha': 'alpha_tags', 'beta': 'beta_tags'}}}]\n";
    ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/settings.py", settings)
        .file(
            "/proj/alpha_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag(name='shared_tag')\ndef alpha(value):\n    pass\n",
        )
        .file(
            "/proj/beta_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag(name='shared_tag')\ndef beta():\n    pass\n",
        )
        .file(
            "/proj/templates/page.html",
            "{% load alpha %}{% shared_tag %}",
        )
        .install(&mut db);

    let errors = collect_file_errors(&db, "/proj/templates/page.html");
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, ValidationError::ExtractedRuleViolation { .. })),
        "the exact loaded alpha contract must remain authoritative over unloaded beta: {errors:?}"
    );
}

#[test]
fn shadowed_template_file_keeps_its_origin_backend_environment() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/first'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alpha_tags'}}},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/second'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'beta_tags'}}},\n]\n";
    ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/settings.py", settings)
        .file(
            "/proj/alpha_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef alpha():\n    pass\n",
        )
        .file(
            "/proj/beta_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef beta():\n    pass\n",
        )
        .file("/proj/first/page.html", "{% load shared %}{% alpha %}")
        .file("/proj/second/page.html", "{% load shared %}{% beta %}")
        .install(&mut db);

    let errors = collect_file_errors(&db, "/proj/second/page.html");
    assert!(
        !errors.iter().any(|error| matches!(
            error,
            ValidationError::UnknownTag { tag, .. }
                | ValidationError::UnloadedTag { tag, .. } if tag == "beta"
        )),
        "a shadowed origin should retain its own backend membership: {errors:?}"
    );
}

#[test]
fn later_load_does_not_change_an_open_block_contract() {
    let mut db = TestDatabase::new();
    ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file(
            "/proj/myproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'custom_tags'}}}]\n",
        )
        .file(
            "/proj/custom_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag(name='if')\ndef custom_if(value):\n    pass\n",
        )
        .file(
            "/proj/templates/page.html",
            "{% if value %}{% load custom %}{% endif %}",
        )
        .install(&mut db);

    let errors = collect_file_errors(&db, "/proj/templates/page.html");
    assert!(
        !errors.iter().any(|error| matches!(
            error,
            ValidationError::UnknownTag { tag, .. }
                | ValidationError::UnloadedTag { tag, .. }
                if tag == "if" || tag == "endif"
        )),
        "the fixture must recognize the builtin if definition: {errors:?}"
    );
    assert!(
        !errors.iter().any(|error| matches!(
            error,
            ValidationError::UnclosedTag { .. }
                | ValidationError::OrphanedClosingTag { .. }
                | ValidationError::UnbalancedStructure { .. }
        )),
        "the opener's captured closer contract must survive later shadowing: {errors:?}"
    );
}

#[test]
fn load_discovery_rebuilds_structure_until_later_load_is_visible() {
    let mut db = TestDatabase::new();
    ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file(
            "/proj/myproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['opaque_tags'], 'libraries': {'first': 'first_tags', 'second': 'second_tags'}}}]\n",
        )
        .file(
            "/proj/opaque_tags.py",
            "from django import template\nregister = template.Library()\n@register.tag(name='shadow')\ndef shadow(parser, token):\n    parser.skip_past('endshadow')\n    return Node()\n",
        )
        .file(
            "/proj/first_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag(name='shadow')\ndef shadow(): pass\n",
        )
        .file(
            "/proj/second_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef revealed(): pass\n",
        )
        .file(
            "/proj/templates/page.html",
            "{% load first %}{% shadow %}{% load second %}{% endshadow %}{% revealed %}",
        )
        .install(&mut db);

    let errors = collect_file_errors(&db, "/proj/templates/page.html");
    assert!(
        !errors.iter().any(|error| matches!(
            error,
            ValidationError::UnloadedTag { tag, .. }
                | ValidationError::UnknownTag { tag, .. }
                if tag == "revealed"
        )),
        "the load revealed after rebuilding structure must activate its library: {errors:?}"
    );
}

#[test]
fn load_discovery_converges_through_more_than_eight_grammar_changes() {
    const REVEAL_COUNT: usize = 10;
    const {
        assert!(REVEAL_COUNT > 8);
    }

    let mut settings_libraries = String::new();
    let mut opaque_tags =
        "from django import template\nregister = template.Library()\n".to_string();
    let mut template = "{% load chain_0 %}".to_string();
    let mut fixture = ProjectFixture::new("/proj").django_settings_module("myproject.settings");

    for index in 0..REVEAL_COUNT {
        if index > 0 {
            settings_libraries.push_str(", ");
        }
        write!(settings_libraries, "'chain_{index}': 'chain_{index}_tags'").unwrap();
        write!(
            opaque_tags,
            "@register.tag(name='gate_{index}')\ndef gate_{index}(parser, token):\n    parser.skip_past('endgate_{index}')\n    return Node()\n"
        )
        .unwrap();
        write!(
            template,
            "{{% gate_{index} %}}{{% load chain_{} %}}{{% endgate_{index} %}}",
            index + 1
        )
        .unwrap();
        fixture = fixture.file(
            format!("/proj/chain_{index}_tags.py"),
            format!(
                "from django import template\nregister = template.Library()\n@register.simple_tag(name='gate_{index}')\ndef gate_{index}(): pass\n"
            ),
        );
    }

    write!(
        settings_libraries,
        ", 'chain_{REVEAL_COUNT}': 'chain_{REVEAL_COUNT}_tags'"
    )
    .unwrap();
    template.push_str("{% revealed %}");
    fixture = fixture
        .file(
            "/proj/myproject/settings.py",
            format!(
                "INSTALLED_APPS = []\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False, 'OPTIONS': {{'builtins': ['opaque_tags'], 'libraries': {{{settings_libraries}}}}}}}]\n"
            ),
        )
        .file("/proj/opaque_tags.py", opaque_tags)
        .file(
            format!("/proj/chain_{REVEAL_COUNT}_tags.py"),
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef revealed(): pass\n",
        )
        .file("/proj/templates/page.html", template);
    let mut db = TestDatabase::new();
    fixture.install(&mut db);

    let errors = collect_file_errors(&db, "/proj/templates/page.html");
    assert!(
        !errors.iter().any(|error| matches!(
            error,
            ValidationError::UnloadedTag { tag, .. }
                | ValidationError::UnknownTag { tag, .. }
                if tag == "revealed"
        )),
        "the final load revealed through the grammar chain must activate its symbol: {errors:?}"
    );
}

#[test]
fn custom_tag_named_if_does_not_run_builtin_if_expression_validation() {
    let mut db = TestDatabase::new();
    ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file(
            "/proj/myproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'custom_tags'}}}]\n",
        )
        .file(
            "/proj/custom_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag(name='if')\ndef custom_if(*args):\n    pass\n",
        )
        .file(
            "/proj/templates/page.html",
            "{% load custom %}{% if and value %}",
        )
        .install(&mut db);

    let errors = collect_file_errors(&db, "/proj/templates/page.html");
    assert!(
        !errors
            .iter()
            .any(|error| matches!(error, ValidationError::ExpressionSyntaxError { .. })),
        "validation behavior must follow the effective role, not the source spelling: {errors:?}"
    );
}

#[test]
fn validation_is_inconclusive_when_feasible_backends_disagree() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nif FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alpha_tags'}}}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'beta_tags'}}}]\n";
    ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/settings.py", settings)
        .file(
            "/proj/alpha_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef alpha():\n    pass\n",
        )
        .file(
            "/proj/beta_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef beta():\n    pass\n",
        )
        .file("/proj/shared/page.html", "{% load shared %}{% alpha %}{% beta %}")
        .install(&mut db);

    let errors = collect_file_errors(&db, "/proj/shared/page.html");
    assert!(!errors.iter().any(|error| matches!(
        error,
        ValidationError::UnknownLibrary { .. }
            | ValidationError::UnknownTag { .. }
            | ValidationError::UnloadedTag { .. }
            | ValidationError::AmbiguousUnloadedTag { .. }
    )));
}

fn alias_shadowing_db(settings: &str, source: &str) -> TestDatabase {
    let mut db = TestDatabase::new();
    ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/settings.py", settings)
        .file(
            "/proj/alias_tags.py",
            "from django import template\nregister = template.Library()\n",
        )
        .file("/proj/available/__init__.py", "")
        .file("/proj/available/templatetags/__init__.py", "")
        .file(
            "/proj/available/templatetags/shared.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef candidate_tag():\n    pass\n@register.filter\ndef candidate_filter(value):\n    return value\n",
        )
        .file("/proj/shared/page.html", source)
        .install(&mut db);
    db
}

#[test]
fn authoritative_aliases_on_all_feasible_backends_suppress_available_app_guidance() {
    let settings = "INSTALLED_APPS = []\nif FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alias_tags'}}}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alias_tags'}}}]\n";
    let db = alias_shadowing_db(settings, "{% candidate_tag %}{{ value|candidate_filter }}");
    let errors = collect_file_errors(&db, "/proj/shared/page.html");

    assert!(!errors.iter().any(|error| matches!(
        error,
        ValidationError::TagNotInInstalledApps { tag, .. } if tag == "candidate_tag"
    )));
    assert!(!errors.iter().any(|error| matches!(
        error,
        ValidationError::FilterNotInInstalledApps { filter, .. } if filter == "candidate_filter"
    )));
}

#[test]
fn mixed_authoritative_alias_shadowing_makes_available_app_guidance_inconclusive() {
    let settings = "INSTALLED_APPS = []\nif FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alias_tags'}}}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/shared'], 'APP_DIRS': False}]\n";
    let db = alias_shadowing_db(settings, "{% candidate_tag %}{{ value|candidate_filter }}");
    let errors = collect_file_errors(&db, "/proj/shared/page.html");

    assert!(
        !errors.iter().any(|error| matches!(
            error,
            ValidationError::TagNotInInstalledApps { tag, .. }
                | ValidationError::UnknownTag { tag, .. } if tag == "candidate_tag"
        )),
        "mixed tag shadowing should remain inconclusive: {errors:?}"
    );
    assert!(
        !errors.iter().any(|error| matches!(
            error,
            ValidationError::FilterNotInInstalledApps { filter, .. }
                | ValidationError::UnknownFilter { filter, .. } if filter == "candidate_filter"
        )),
        "mixed filter shadowing should remain inconclusive: {errors:?}"
    );
}

fn extracted_block_db(source: &str) -> TestDatabase {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/blog/__init__.py", "")
        .file("/proj/blog/templatetags/__init__.py", "")
        .file("/proj/blog/templatetags/ambiguous.py", source)
        .file("/proj/django/__init__.py", "")
        .file("/proj/django/template/__init__.py", "")
        .file(
            "/proj/django/template/defaulttags.py",
            "from django import template\nregister = template.Library()\n@register.tag\ndef load(parser, token): pass\n",
        )
        .file(
            "/proj/django/template/loader_tags.py",
            "from django import template\nregister = template.Library()\n@register.tag\ndef block(parser, token): pass\n@register.tag\ndef extends(parser, token): pass\n@register.tag\ndef include(parser, token): pass\n",
        )
        .file(
            "/proj/myproject/settings.py",
            "INSTALLED_APPS = ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n",
        )
        .install(&mut db);

    let specs = compute_tag_specs(&db, project).clone();
    db.with_specs(specs)
}

fn extracted_unknown_block_db() -> TestDatabase {
    let source = r#"
from django import template

register = template.Library()

@register.tag("mystery")
def do_mystery(parser, token):
    options = {"name": "mystery"}
    nodelist = parser.parse((f"end{options['name']}",))
    return MysteryNode(nodelist)
"#;

    extracted_block_db(source)
}

fn extracted_self_named_block_db() -> TestDatabase {
    let source = r#"
from django import template

register = template.Library()

@register.tag("mystery")
def do_mystery(parser, token):
    tag_name, *rest = token.split_contents()
    nodelist = parser.parse((f"end{tag_name}",))
    return MysteryNode(nodelist)
"#;

    extracted_block_db(source)
}

#[test]
fn extracted_unknown_block_does_not_require_synthesized_end_tag() {
    let db = extracted_unknown_block_db();
    assert_eq!(
        db.tag_specs()
            .get("mystery")
            .and_then(|spec| spec.end_tag.as_ref())
            .map(|end_tag| end_tag.name.as_ref()),
        None::<&str>,
        "ambiguous extracted closer must stay unknown, not be synthesized"
    );

    let errors = collect_all_errors(&db, "{% load ambiguous %}\n{% mystery %}\n");

    assert!(
        !errors.iter().any(|error| matches!(
            error,
            ValidationError::UnclosedTag { tag, .. } if tag == "mystery"
        ) || matches!(
            error,
            ValidationError::UnbalancedStructure { opening_tag, .. } if opening_tag == "mystery"
        )),
        "extracted dynamic block tags should not require a synthesized closer: {errors:?}"
    );
}

#[test]
fn extracted_self_named_block_requires_concretized_end_tag() {
    let db = extracted_self_named_block_db();
    assert_eq!(
        db.tag_specs()
            .get("mystery")
            .and_then(|spec| spec.end_tag.as_ref())
            .map(|end_tag| end_tag.name.as_ref()),
        Some("endmystery")
    );

    let errors = collect_all_errors(&db, "{% load ambiguous %}\n{% mystery %}\n");

    assert!(
        errors.iter().any(|error| matches!(
            error,
            ValidationError::UnclosedTag { tag, .. } if tag == "mystery" && error.code() == "S100"
        )),
        "self-named extracted block tags should require their evidenced closer: {errors:?}"
    );
}

#[test]
fn partial_knowledge_suppresses_unknown_tag() {
    let db = partial_db();
    let errors = collect_all_errors(&db, "{% definitely_unknown %}\n");

    assert!(
        !errors
            .iter()
            .any(|error| matches!(error, ValidationError::UnknownTag { .. })),
        "unknown tags should be suppressed under partial knowledge: {errors:?}"
    );
}

#[test]
fn unknown_loaded_library_suppresses_unloaded_tag_and_filter_diagnostics() {
    let db = partial_ambiguous_db();
    let errors = collect_all_errors(
        &db,
        "{% load unknown_library %}\n{% shared %}\n{{ value|shared }}\n",
    );

    assert!(
        !errors.iter().any(|error| matches!(
            error,
            ValidationError::UnloadedTag { tag, .. }
                | ValidationError::AmbiguousUnloadedTag { tag, .. } if tag == "shared"
        )),
        "an unknown full load may provide the known tag: {errors:?}"
    );
    assert!(
        !errors.iter().any(|error| matches!(
            error,
            ValidationError::UnloadedFilter { filter, .. }
                | ValidationError::AmbiguousUnloadedFilter { filter, .. } if filter == "shared"
        )),
        "an unknown full load may provide the known filter: {errors:?}"
    );
}

#[test]
fn partial_knowledge_suppresses_unknown_load_library() {
    let db = partial_db();
    let errors = collect_all_errors(&db, "{% load missing_library %}\n");

    assert!(
        !errors
            .iter()
            .any(|error| matches!(error, ValidationError::UnknownLibrary { .. })),
        "unknown load libraries should be suppressed under partial knowledge: {errors:?}"
    );
}

#[test]
fn partial_knowledge_suppresses_unknown_filter() {
    let db = partial_db();
    let errors = collect_all_errors(&db, "{{ value|definitely_unknown }}\n");

    assert!(
        !errors
            .iter()
            .any(|error| matches!(error, ValidationError::UnknownFilter { .. })),
        "unknown filters should be suppressed under partial knowledge: {errors:?}"
    );
}

#[test]
fn partial_knowledge_suppresses_filter_arity_after_unknown_load() {
    let db = partial_db();
    let errors = collect_all_errors(
        &db,
        "{% load project_filters %}\n{{ value|truncatewords }}\n",
    );

    assert!(
        !errors.iter().any(|error| matches!(
            error,
            ValidationError::FilterMissingArgument { filter, .. } if filter == "truncatewords"
        )),
        "unknown loaded libraries may shadow known filters under partial knowledge: {errors:?}"
    );
}

#[test]
fn unknown_load_name_without_available_candidate_stays_unknown_library() {
    let db = standard_db();
    let errors = collect_all_errors(&db, "{% load missing_library %}\n");

    assert!(
        errors.iter().any(|error| matches!(
            error,
            ValidationError::UnknownLibrary { name, .. } if name == "missing_library"
        )),
        "missing library should keep S120: {errors:?}"
    );
}

#[test]
fn unknown_tag_without_available_candidate_stays_unknown_tag() {
    let db = standard_db();
    let errors = collect_all_errors(&db, "{% definitely_unknown %}\n");

    assert!(
        errors.iter().any(|error| matches!(
            error,
            ValidationError::UnknownTag { tag, .. } if tag == "definitely_unknown"
        )),
        "unknown tag should keep S108: {errors:?}"
    );
}

// Integration: Mixed diagnostics

#[test]
fn mixed_expression_and_filter_arity_errors() {
    let db = standard_db();
    let source = concat!(
        "{% if and x %}bad expr{% endif %}\n",
        "{{ value|truncatewords }}\n",
        "{{ value|title:\"bad\" }}\n",
    );
    let errors = collect_all_errors(&db, source);

    let expr_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, ValidationError::ExpressionSyntaxError { .. }))
        .collect();
    let s115_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, ValidationError::FilterMissingArgument { .. }))
        .collect();
    let s116_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, ValidationError::FilterUnexpectedArgument { .. }))
        .collect();

    assert_eq!(
        expr_errors.len(),
        1,
        "Expected 1 expression error, got: {expr_errors:?}"
    );
    assert_eq!(
        s115_errors.len(),
        1,
        "Expected 1 FilterMissingArgument, got: {s115_errors:?}"
    );
    assert_eq!(
        s116_errors.len(),
        1,
        "Expected 1 FilterUnexpectedArgument, got: {s116_errors:?}"
    );
}

#[test]
fn opaque_region_suppresses_all_validation() {
    let db = standard_db();
    // Everything inside verbatim should be skipped
    let source = concat!(
        "{% verbatim %}\n",
        "{% if and x %}bad expr{% endif %}\n",
        "{{ value|truncatewords }}\n",
        "{{ value|title:\"bad\" }}\n",
        "{% endverbatim %}\n",
    );
    let errors = collect_all_errors(&db, source);

    // Filter out structural errors (UnclosedTag etc) that come from the block tree
    let validation_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                ValidationError::ExpressionSyntaxError { .. }
                    | ValidationError::FilterMissingArgument { .. }
                    | ValidationError::FilterUnexpectedArgument { .. }
                    | ValidationError::UnknownTag { .. }
                    | ValidationError::UnloadedTag { .. }
                    | ValidationError::UnknownFilter { .. }
                    | ValidationError::UnloadedFilter { .. }
            )
        })
        .collect();

    assert!(
        validation_errors.is_empty(),
        "No expression/filter/scoping errors expected inside verbatim, got: {validation_errors:?}"
    );
}

#[test]
fn errors_before_and_after_opaque_region() {
    let db = standard_db();
    let source = concat!(
        "{{ value|truncatewords }}\n",
        "{% verbatim %}{% if and x %}{% endverbatim %}\n",
        "{{ value|title:\"bad\" }}\n",
    );
    let errors = collect_all_errors(&db, source);

    let s115_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, ValidationError::FilterMissingArgument { .. }))
        .collect();
    let s116_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, ValidationError::FilterUnexpectedArgument { .. }))
        .collect();
    let expr_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, ValidationError::ExpressionSyntaxError { .. }))
        .collect();

    assert_eq!(
        s115_errors.len(),
        1,
        "Expected S115 before verbatim, got: {s115_errors:?}"
    );
    assert_eq!(
        s116_errors.len(),
        1,
        "Expected S116 after verbatim, got: {s116_errors:?}"
    );
    assert!(
        expr_errors.is_empty(),
        "No expression errors expected (bad if is inside verbatim), got: {expr_errors:?}"
    );
}

#[test]
fn comment_block_also_opaque() {
    let db = standard_db();
    let source = concat!(
        "{% comment %}\n",
        "{% if and x %}{% endif %}\n",
        "{{ value|truncatewords }}\n",
        "{% endcomment %}\n",
    );
    let errors = collect_all_errors(&db, source);

    let validation_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                ValidationError::ExpressionSyntaxError { .. }
                    | ValidationError::FilterMissingArgument { .. }
                    | ValidationError::FilterUnexpectedArgument { .. }
            )
        })
        .collect();

    assert!(
        validation_errors.is_empty(),
        "No errors expected inside comment block, got: {validation_errors:?}"
    );
}

#[test]
fn load_inside_block_affects_later_occurrences() {
    let db = standard_db();
    let errors = collect_all_errors(
        &db,
        "{% if value %}{% load i18n %}{% trans 'hello' %}{% endif %}",
    );

    assert!(
        !errors.iter().any(|error| matches!(
            error,
            ValidationError::UnloadedTag { tag, .. } if tag == "trans"
        )),
        "an active nested load should affect later occurrences: {errors:?}"
    );
}

#[test]
fn load_inside_verbatim_does_not_affect_later_tag_availability() {
    let db = standard_db();
    let source = concat!(
        "{% verbatim %}{% load i18n %}{% endverbatim %}\n",
        "{% trans \"hello\" %}\n",
    );
    let errors = collect_all_errors(&db, source);

    assert!(
        errors.iter().any(|error| matches!(
            error,
            ValidationError::UnloadedTag { tag, library, .. }
                if tag == "trans" && library == "i18n"
        )),
        "opaque load should not make trans available: {errors:?}"
    );
}

#[test]
fn load_inside_comment_does_not_affect_later_tag_availability() {
    let db = standard_db();
    let source = concat!(
        "{% comment %}{% load i18n %}{% endcomment %}\n",
        "{% trans \"hello\" %}\n",
    );
    let errors = collect_all_errors(&db, source);

    assert!(
        errors.iter().any(|error| matches!(
            error,
            ValidationError::UnloadedTag { tag, library, .. }
                if tag == "trans" && library == "i18n"
        )),
        "opaque load should not make trans available: {errors:?}"
    );
}

#[test]
fn unloaded_tag_and_filter_with_expression_error() {
    let db = standard_db();
    // trans requires {% load i18n %}, but it's not loaded
    // Also has an expression error in an if tag
    let source = concat!("{% if or x %}bad{% endif %}\n", "{% trans \"hello\" %}\n",);
    let errors = collect_all_errors(&db, source);

    let expr_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, ValidationError::ExpressionSyntaxError { .. }))
        .collect();
    let scoping_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                ValidationError::UnloadedTag { .. } | ValidationError::UnknownTag { .. }
            )
        })
        .collect();

    assert_eq!(
        expr_errors.len(),
        1,
        "Expected 1 expression error, got: {expr_errors:?}"
    );
    assert_eq!(
        scoping_errors.len(),
        1,
        "Expected 1 scoping error for trans, got: {scoping_errors:?}"
    );
    // Verify it's specifically an UnloadedTag for trans
    assert!(
        matches!(&scoping_errors[0], ValidationError::UnloadedTag { tag, library, .. }
            if tag == "trans" && library == "i18n"),
        "Expected UnloadedTag for trans/i18n, got: {:?}",
        scoping_errors[0]
    );
}

#[test]
fn loaded_library_tags_valid_with_filter_errors() {
    let db = standard_db();
    let source = concat!(
        "{% load i18n %}\n",
        "{% trans \"hello\" %}\n",
        "{{ value|truncatewords }}\n",
    );
    let errors = collect_all_errors(&db, source);

    let scoping_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                ValidationError::UnloadedTag { .. } | ValidationError::UnknownTag { .. }
            )
        })
        .collect();
    let s115_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, ValidationError::FilterMissingArgument { .. }))
        .collect();

    assert!(
        scoping_errors.is_empty(),
        "No scoping errors after load, got: {scoping_errors:?}"
    );
    assert_eq!(
        s115_errors.len(),
        1,
        "Expected S115 for truncatewords, got: {s115_errors:?}"
    );
}

// Snapshot tests for diagnostic output

#[test]
fn snapshot_mixed_diagnostics() {
    let db = standard_db();
    let source = concat!(
        "{% if and x %}oops{% endif %}\n",
        "{{ name|title:\"arg\" }}\n",
        "{{ text|truncatewords }}\n",
        "{% trans \"hello\" %}\n",
    );

    let rendered = djls_testing::render_validate_snapshot(&db, "test.html", 0, source);
    insta::assert_snapshot!(rendered);
}

#[test]
fn snapshot_clean_template_no_errors() {
    let db = standard_db();
    let source = concat!(
        "{% if user.is_authenticated %}\n",
        "  <h1>{{ user.name|title }}</h1>\n",
        "  {{ user.joined|date:\"Y-m-d\" }}\n",
        "{% endif %}\n",
    );
    let errors = collect_all_errors(&db, source);

    let validation_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                ValidationError::ExpressionSyntaxError { .. }
                    | ValidationError::FilterMissingArgument { .. }
                    | ValidationError::FilterUnexpectedArgument { .. }
                    | ValidationError::UnknownTag { .. }
                    | ValidationError::UnloadedTag { .. }
                    | ValidationError::UnknownFilter { .. }
                    | ValidationError::UnloadedFilter { .. }
            )
        })
        .collect();

    assert!(
        validation_errors.is_empty(),
        "Clean template should produce no validation errors, got: {validation_errors:?}"
    );
}

#[test]
fn snapshot_complex_valid_template() {
    let db = standard_db();
    // A realistic Django admin-style template with various features
    let source = concat!(
        "{% load i18n %}\n",
        "{% if user.is_staff and not user.is_superuser %}\n",
        "  <p>{{ greeting|default:\"Hello\" }}</p>\n",
        "  {% for item in items %}\n",
        "    <li>{{ item.name|title }} - {{ item.date|date }}</li>\n",
        "  {% endfor %}\n",
        "  {% trans \"Welcome\" %}\n",
        "{% endif %}\n",
        "{% verbatim %}\n",
        "  {{ raw_template_syntax }}\n",
        "{% endverbatim %}\n",
    );
    let errors = collect_all_errors(&db, source);

    let validation_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                ValidationError::ExpressionSyntaxError { .. }
                    | ValidationError::FilterMissingArgument { .. }
                    | ValidationError::FilterUnexpectedArgument { .. }
                    | ValidationError::UnknownTag { .. }
                    | ValidationError::UnloadedTag { .. }
                    | ValidationError::UnknownFilter { .. }
                    | ValidationError::UnloadedFilter { .. }
            )
        })
        .collect();

    assert!(
        validation_errors.is_empty(),
        "Valid complex template should have no errors, got: {validation_errors:?}"
    );
}

#[test]
fn snapshot_multiple_error_types() {
    let db = standard_db();
    let source = concat!(
        "{{ value|title:\"unwanted\" }}\n",
        "{% if == broken %}bad{% endif %}\n",
        "{{ text|lower:\"arg\" }}\n",
        "{% comment %}{% if and %}{% endcomment %}\n",
        "{{ result|truncatewords }}\n",
    );

    let rendered = djls_testing::render_validate_snapshot(&db, "test.html", 0, source);
    insta::assert_snapshot!(rendered);
}

// Extends validation (S122, S123)

#[test]
fn extends_as_first_tag_no_errors() {
    let db = standard_db();
    let source = r#"{% extends "base.html" %}"#;
    let errors = collect_all_errors(&db, source);
    let extends_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                ValidationError::ExtendsMustBeFirst { .. }
                    | ValidationError::MultipleExtends { .. }
            )
        })
        .collect();
    assert!(
        extends_errors.is_empty(),
        "No extends errors expected, got: {extends_errors:?}"
    );
}

#[test]
fn text_whitespace_before_extends_no_errors() {
    let db = standard_db();
    let source = "  \n\n  {% extends \"base.html\" %}";
    let errors = collect_all_errors(&db, source);
    let extends_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                ValidationError::ExtendsMustBeFirst { .. }
                    | ValidationError::MultipleExtends { .. }
            )
        })
        .collect();
    assert!(
        extends_errors.is_empty(),
        "Text/whitespace before extends should be fine, got: {extends_errors:?}"
    );
}

#[test]
fn comment_before_extends_no_errors() {
    let db = standard_db();
    let source = "{# this is a comment #}{% extends \"base.html\" %}";
    let errors = collect_all_errors(&db, source);
    let extends_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                ValidationError::ExtendsMustBeFirst { .. }
                    | ValidationError::MultipleExtends { .. }
            )
        })
        .collect();
    assert!(
        extends_errors.is_empty(),
        "Comment before extends should be fine, got: {extends_errors:?}"
    );
}

#[test]
fn no_extends_at_all_no_errors() {
    let db = standard_db();
    let source = "{% if user %}hello{% endif %}";
    let errors = collect_all_errors(&db, source);
    let extends_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            matches!(
                e,
                ValidationError::ExtendsMustBeFirst { .. }
                    | ValidationError::MultipleExtends { .. }
            )
        })
        .collect();
    assert!(
        extends_errors.is_empty(),
        "No extends = no extends errors, got: {extends_errors:?}"
    );
}

#[test]
fn tag_before_extends_s122() {
    let db = standard_db();
    let source = "{% load i18n %}{% extends \"base.html\" %}";
    let errors = collect_all_errors(&db, source);
    let s122: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, ValidationError::ExtendsMustBeFirst { .. }))
        .collect();
    assert_eq!(s122.len(), 1, "Expected S122, got: {s122:?}");
}

#[test]
fn variable_before_extends_s122() {
    let db = standard_db();
    let source = "{{ variable }}{% extends \"base.html\" %}";
    let errors = collect_all_errors(&db, source);
    let s122: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, ValidationError::ExtendsMustBeFirst { .. }))
        .collect();
    assert_eq!(s122.len(), 1, "Expected S122, got: {s122:?}");
}

#[test]
fn multiple_extends_s123() {
    let db = standard_db();
    let source = r#"{% extends "base.html" %}{% extends "other.html" %}"#;
    let errors = collect_all_errors(&db, source);
    let s123: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, ValidationError::MultipleExtends { .. }))
        .collect();
    assert_eq!(s123.len(), 1, "Expected S123, got: {s123:?}");
    // First extends should NOT produce S122
    let s122: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, ValidationError::ExtendsMustBeFirst { .. }))
        .collect();
    assert!(s122.is_empty(), "First extends is valid, got: {s122:?}");
}

#[test]
fn tag_before_extends_and_multiple_extends_s122_and_s123() {
    let db = standard_db();
    let source = r#"{% load i18n %}{% extends "a.html" %}{% extends "b.html" %}"#;
    let errors = collect_all_errors(&db, source);
    let s122: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, ValidationError::ExtendsMustBeFirst { .. }))
        .collect();
    let s123: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, ValidationError::MultipleExtends { .. }))
        .collect();
    assert_eq!(s122.len(), 1, "Expected S122, got: {s122:?}");
    assert_eq!(s123.len(), 1, "Expected S123, got: {s123:?}");
}

#[test]
fn extends_inside_verbatim_after_content_does_not_need_to_be_first() {
    let db = standard_db();
    let source = r#"<p>body</p>{% verbatim %}{% extends "base.html" %}{% endverbatim %}"#;
    let errors = collect_all_errors(&db, source);
    let s122: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, ValidationError::ExtendsMustBeFirst { .. }))
        .collect();

    assert!(
        s122.is_empty(),
        "Extends inside verbatim should not affect extends ordering, got: {s122:?}"
    );
}

#[test]
fn multiple_extends_inside_comment_do_not_count_as_multiple_extends() {
    let db = standard_db();
    let source = r#"{% comment %}{% extends "a.html" %}{% extends "b.html" %}{% endcomment %}"#;
    let errors = collect_all_errors(&db, source);
    let s123: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, ValidationError::MultipleExtends { .. }))
        .collect();

    assert!(
        s123.is_empty(),
        "Extends inside comment should not count as active extends tags, got: {s123:?}"
    );
}

#[test]
fn opaque_extends_after_active_extends_does_not_count_as_second_extends() {
    let db = standard_db();
    let source =
        r#"{% extends "base.html" %}{% verbatim %}{% extends "ignored.html" %}{% endverbatim %}"#;
    let errors = collect_all_errors(&db, source);
    let s123: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, ValidationError::MultipleExtends { .. }))
        .collect();

    assert!(
        s123.is_empty(),
        "Extends inside verbatim should not count as a second active extends tag, got: {s123:?}"
    );
}

// Corpus / template validation tests
//
// These tests extract rules from real Django source files and validate
// real templates against those rules, proving zero false positives for
// argument validation (S114, S115, S116, S117) at scale.
//
// All tests skip gracefully when the corpus is unavailable.
// Run `cargo run -p djls-testing --bin corpus -- sync` to populate it.

use djls_testing::Corpus;
use djls_testing::build_entry_specs;
use djls_testing::build_specs_from_extraction;
use djls_testing::collect_argument_validation_errors_with_revision;

struct FailureEntry {
    path: Utf8PathBuf,
    errors: Vec<String>,
}

fn format_failures(failures: &[FailureEntry]) -> String {
    let mut out = String::new();
    for f in failures.iter().take(20) {
        let _ = writeln!(out, "  {}:", f.path);
        for err in &f.errors {
            let _ = writeln!(out, "    - {err}");
        }
    }
    if failures.len() > 20 {
        let _ = writeln!(out, "  ... and {} more", failures.len() - 20);
    }
    out
}

#[test]
fn corpus_templates_have_no_argument_false_positives() {
    let corpus = Corpus::require();

    let templates = corpus.templates_in(corpus.root());
    let mut by_entry: BTreeMap<Utf8PathBuf, Vec<Utf8PathBuf>> = BTreeMap::new();

    for template_path in templates {
        let Some(entry_dir) = corpus.entry_dir_for_path(&template_path) else {
            continue;
        };

        by_entry.entry(entry_dir).or_default().push(template_path);
    }

    for templates in by_entry.values_mut() {
        templates.sort();
    }

    let mut failures = Vec::new();

    for (entry_dir, templates) in by_entry {
        if templates.is_empty() {
            continue;
        }

        let (specs, arities) = build_entry_specs(&corpus, &entry_dir);
        let db = TestDatabase::new()
            .with_specs(specs)
            .with_arity_specs(arities);

        for (i, template_path) in templates.into_iter().enumerate() {
            let Ok(content) = std::fs::read_to_string(template_path.as_std_path()) else {
                continue;
            };

            let errors = collect_argument_validation_errors_with_revision(
                &db,
                "corpus_test.html",
                i as u64,
                &content,
            );
            if errors.is_empty() {
                continue;
            }

            failures.push(FailureEntry {
                path: template_path,
                errors: errors
                    .into_iter()
                    .take(5)
                    .map(|e| format!("{e:?}"))
                    .collect(),
            });
        }
    }

    assert!(
        failures.is_empty(),
        "Corpus templates have false positives:\n{}",
        format_failures(&failures)
    );
}

#[test]
fn corpus_known_invalid_templates_produce_errors() {
    let corpus = Corpus::require();

    let Some(django_dir) = corpus.latest_package("django") else {
        eprintln!("No Django in corpus.");
        return;
    };

    let (specs, arities) = build_specs_from_extraction(&corpus, &django_dir);

    let db = TestDatabase::new()
        .with_specs(specs)
        .with_arity_specs(arities);

    // for tag with wrong number of args
    let errors = collect_argument_validation_errors_with_revision(
        &db,
        "corpus_test.html",
        0,
        "{% for %}content{% endfor %}",
    );
    assert!(
        !errors.is_empty(),
        "Expected errors for {{% for %}} with no args"
    );

    // if expression syntax error
    let errors = collect_argument_validation_errors_with_revision(
        &db,
        "corpus_test.html",
        1,
        "{% if and x %}content{% endif %}",
    );
    let expr_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, ValidationError::ExpressionSyntaxError { .. }))
        .collect();
    assert!(
        !expr_errors.is_empty(),
        "Expected expression syntax error for {{% if and x %}}"
    );
}

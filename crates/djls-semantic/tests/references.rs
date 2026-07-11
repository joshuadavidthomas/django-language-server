use camino::Utf8Path;
use djls_project::Project;
use djls_project::TemplateName;
use djls_semantic::TemplateReferenceKind;
use djls_semantic::references_to_template_name;
use djls_semantic::template_library_references_in_file;
use djls_source::ChangeEvent;
use djls_source::SourceChanges;
use djls_source::Span;
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
        .install(db)
}

#[test]
fn inconclusive_load_role_does_not_create_a_load_event() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nif FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'custom_tags'}}}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['custom_load'], 'libraries': {'custom': 'custom_tags'}}}]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/custom_load.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag(name='load')\ndef custom_load(value): pass\n",
        )
        .file(
            "/test/project/custom_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag(name='include')\ndef custom_include(value): pass\n",
        )
        .file(
            "/test/project/templates/child.html",
            "{% load custom %}{% include 'partial.html' %}",
        )
        .file("/test/project/templates/partial.html", "partial")
        .install(&mut db);
    let partial = TemplateName::new(&db, "partial.html".to_string());

    assert_eq!(references_to_template_name(&db, project, partial).len(), 1);
}

#[test]
fn template_references_record_extends_and_include_kinds() {
    let mut db = TestDatabase::new();
    let project = project_with_templates(
        &mut db,
        vec!["/test/project/templates"],
        vec![
            (
                "child.html",
                "/test/project/templates/child.html",
                "{% extends \"base.html\" %}\n{% include \"partial.html\" %}",
            ),
            ("base.html", "/test/project/templates/base.html", "base"),
            (
                "partial.html",
                "/test/project/templates/partial.html",
                "partial",
            ),
        ],
    );

    let base = TemplateName::new(&db, "base.html".to_string());
    let partial = TemplateName::new(&db, "partial.html".to_string());

    let base_refs = references_to_template_name(&db, project, base);
    let partial_refs = references_to_template_name(&db, project, partial);

    assert_eq!(base_refs.len(), 1);
    assert_eq!(base_refs[0].kind(&db), TemplateReferenceKind::Extends);
    assert_eq!(
        base_refs[0].span(&db),
        Span::saturating_from_parts_usize(12, 9)
    );
    assert_eq!(partial_refs.len(), 1);
    assert_eq!(partial_refs[0].kind(&db), TemplateReferenceKind::Include);
}

#[test]
fn later_load_only_shadows_template_reference_occurrences_after_it() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'custom_tags'}}}]\n",
        )
        .file(
            "/test/project/custom_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag(name='include')\ndef custom_include(value):\n    pass\n",
        )
        .file(
            "/test/project/templates/child.html",
            "{% include 'partial.html' %}\n{% load custom %}\n{% include 'partial.html' %}",
        )
        .file("/test/project/templates/partial.html", "partial")
        .install(&mut db);

    let partial = TemplateName::new(&db, "partial.html".to_string());
    let references = references_to_template_name(&db, project, partial);

    assert_eq!(references.len(), 1);
    assert_eq!(references[0].kind(&db), TemplateReferenceKind::Include);
    assert_eq!(references[0].span(&db).start(), 12);
}

#[test]
fn unreadable_referencing_template_contributes_no_references() {
    let mut db = TestDatabase::new();
    let child_path = "/test/project/templates/child.html";
    let project = project_with_templates(
        &mut db,
        vec!["/test/project/templates"],
        vec![
            ("child.html", child_path, "{% include 'partial.html' %}"),
            (
                "partial.html",
                "/test/project/templates/partial.html",
                "partial",
            ),
        ],
    );
    {
        let partial = TemplateName::new(&db, "partial.html".to_string());
        assert_eq!(references_to_template_name(&db, project, partial).len(), 1);
    }

    db.remove_file(child_path);
    SourceChanges::new([ChangeEvent::Rescan]).apply(&mut db);

    let partial = TemplateName::new(&db, "partial.html".to_string());
    assert!(references_to_template_name(&db, project, partial).is_empty());
}

#[test]
fn template_references_ignore_dynamic_template_names() {
    let mut db = TestDatabase::new();
    let project = project_with_templates(
        &mut db,
        vec!["/test/project/templates"],
        vec![
            (
                "child.html",
                "/test/project/templates/child.html",
                "{% include partial_name %}\n{% include \"partial.html\" %}",
            ),
            (
                "partial.html",
                "/test/project/templates/partial.html",
                "partial",
            ),
        ],
    );

    let partial = TemplateName::new(&db, "partial.html".to_string());
    let references = references_to_template_name(&db, project, partial);

    assert_eq!(references.len(), 1);
    assert_eq!(references[0].kind(&db), TemplateReferenceKind::Include);
}

#[test]
fn template_references_ignore_include_inside_verbatim() {
    let mut db = TestDatabase::new();
    let project = project_with_templates(
        &mut db,
        vec!["/test/project/templates"],
        vec![
            (
                "child.html",
                "/test/project/templates/child.html",
                "{% verbatim %}{% include \"partial.html\" %}{% endverbatim %}",
            ),
            (
                "partial.html",
                "/test/project/templates/partial.html",
                "partial",
            ),
        ],
    );

    let partial = TemplateName::new(&db, "partial.html".to_string());
    let references = references_to_template_name(&db, project, partial);

    assert!(references.is_empty());
}

#[test]
fn template_references_ignore_extends_inside_comment() {
    let mut db = TestDatabase::new();
    let project = project_with_templates(
        &mut db,
        vec!["/test/project/templates"],
        vec![
            (
                "child.html",
                "/test/project/templates/child.html",
                "{% comment %}{% extends \"base.html\" %}{% endcomment %}",
            ),
            ("base.html", "/test/project/templates/base.html", "base"),
        ],
    );

    let base = TemplateName::new(&db, "base.html".to_string());
    let references = references_to_template_name(&db, project, base);

    assert!(references.is_empty());
}

#[test]
fn template_references_include_only_active_references() {
    let mut db = TestDatabase::new();
    let project = project_with_templates(
        &mut db,
        vec!["/test/project/templates"],
        vec![
            (
                "child.html",
                "/test/project/templates/child.html",
                concat!(
                    "{% verbatim %}{% include \"partial.html\" %}{% endverbatim %}\n",
                    "{% include \"partial.html\" %}",
                ),
            ),
            (
                "partial.html",
                "/test/project/templates/partial.html",
                "partial",
            ),
        ],
    );

    let partial = TemplateName::new(&db, "partial.html".to_string());
    let references = references_to_template_name(&db, project, partial);

    assert_eq!(references.len(), 1);
    assert_eq!(references[0].kind(&db), TemplateReferenceKind::Include);
}

#[test]
fn template_references_exclude_missing_targets() {
    let mut db = TestDatabase::new();
    let project = project_with_templates(
        &mut db,
        vec!["/test/project/templates"],
        vec![(
            "child.html",
            "/test/project/templates/child.html",
            "{% include 'missing.html' %}",
        )],
    );

    let missing = TemplateName::new(&db, "missing.html".to_string());

    assert!(references_to_template_name(&db, project, missing).is_empty());
}

#[test]
fn template_references_keep_known_inconclusive_targets() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [UNKNOWN, '/test/project/templates'], 'APP_DIRS': False}]\n",
        )
        .file(
            "/test/project/templates/child.html",
            "{% include 'partial.html' %}",
        )
        .file("/test/project/templates/partial.html", "partial")
        .install(&mut db);
    let partial = TemplateName::new(&db, "partial.html".to_string());

    let references = references_to_template_name(&db, project, partial);
    assert_eq!(references.len(), 1);
    assert_eq!(references[0].kind(&db), TemplateReferenceKind::Include);
}

#[test]
fn template_references_are_omitted_when_target_exists_only_in_another_backend() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/a'], 'APP_DIRS': False},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/b'], 'APP_DIRS': False},\n]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file("/test/project/a/child.html", "{% include 'only-b.html' %}")
        .file("/test/project/b/only-b.html", "b")
        .install(&mut db);

    let only_b = TemplateName::new(&db, "only-b.html".to_string());
    assert!(references_to_template_name(&db, project, only_b).is_empty());
}

#[test]
fn relative_references_normalize_for_every_name_of_the_source_file() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates', '/test/project/templates/alias'], 'APP_DIRS': False}]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/templates/alias/child.html",
            "{% include './parent.html' %}",
        )
        .file("/test/project/templates/alias/parent.html", "parent")
        .install(&mut db);

    let nested = TemplateName::new(&db, "alias/parent.html".to_string());
    let root = TemplateName::new(&db, "parent.html".to_string());
    assert_eq!(references_to_template_name(&db, project, nested).len(), 1);
    assert_eq!(references_to_template_name(&db, project, root).len(), 1);
}

#[test]
fn custom_shadowed_load_tag_creates_no_library_reference() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['custom_load'], 'libraries': {'custom': 'custom_tags'}}}]\n";
    let _project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/custom_load.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag(name='load')\ndef custom_load(value): pass\n",
        )
        .file(
            "/test/project/custom_tags.py",
            "from django import template\nregister = template.Library()\n",
        )
        .file("/test/project/templates/page.html", "{% load custom %}")
        .install(&mut db);
    let file = db.file(Utf8Path::new("/test/project/templates/page.html"));

    assert!(
        template_library_references_in_file(&db, file)
            .as_slice(&db)
            .is_empty()
    );
}

#[test]
fn shadowed_load_does_not_bootstrap_loaded_opaque_grammar() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['custom_load'], 'libraries': {'custom': 'custom_tags'}}}]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/custom_load.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag(name='load')\ndef custom_load(value): pass\n",
        )
        .file(
            "/test/project/custom_tags.py",
            "from django import template\nregister = template.Library()\n@register.tag(name='shadow')\ndef shadow(parser, token):\n    parser.skip_past('endshadow')\n    return Node()\n",
        )
        .file(
            "/test/project/templates/page.html",
            "{% load custom %}{% shadow %}{% include 'partial.html' %}{% endshadow %}",
        )
        .file("/test/project/templates/partial.html", "partial")
        .install(&mut db);
    let partial = TemplateName::new(&db, "partial.html".to_string());

    assert_eq!(references_to_template_name(&db, project, partial).len(), 1);
}

#[test]
fn template_references_to_template_name_include_all_sources() {
    let mut db = TestDatabase::new();
    let project = project_with_templates(
        &mut db,
        vec!["/test/project/templates"],
        vec![
            (
                "first.html",
                "/test/project/templates/first.html",
                "{% include \"partial.html\" %}",
            ),
            (
                "second.html",
                "/test/project/templates/second.html",
                "{% include \"partial.html\" %}",
            ),
            (
                "partial.html",
                "/test/project/templates/partial.html",
                "partial",
            ),
        ],
    );

    let partial = TemplateName::new(&db, "partial.html".to_string());
    let references = references_to_template_name(&db, project, partial);
    let source_paths: Vec<_> = references
        .iter()
        .map(|reference| reference.source_file(&db).path(&db).as_str())
        .collect();

    assert_eq!(
        source_paths,
        [
            "/test/project/templates/first.html",
            "/test/project/templates/second.html"
        ]
    );
}

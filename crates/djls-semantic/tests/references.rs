use djls_project::Project;
use djls_project::TemplateName;
use djls_semantic::TemplateReferenceKind;
use djls_semantic::references_to_template_name;
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
        .fold(fixture, |fixture, (name, path, source)| {
            fixture.template_file(name, path, source)
        })
        .build(db)
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

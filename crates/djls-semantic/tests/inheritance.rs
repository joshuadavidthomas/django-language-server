use std::borrow::Cow;

use camino::Utf8Path;
use djls_semantic::BlockDef;
use djls_semantic::ChainEnd;
use djls_semantic::EndTag;
use djls_semantic::ExtendsTarget;
use djls_semantic::PartialDef;
use djls_semantic::TagRole;
use djls_semantic::TagSpec;
use djls_semantic::TagSpecs;
use djls_semantic::TemplateSymbols;
use djls_semantic::block_overrides;
use djls_semantic::builtin_tag_specs;
use djls_semantic::inherited_blocks;
use djls_semantic::template_inheritance;
use djls_semantic::template_symbols;
use djls_source::ChangeEvent;
use djls_source::File;
use djls_source::SourceChanges;
use djls_source::Span;
use djls_templates::parse_template;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;
use rustc_hash::FxHashMap;

fn project_with_templates(
    db: &TestDatabase,
    template_dirs: Vec<&str>,
    templates: Vec<(&str, &str)>,
) -> djls_project::Project {
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
        .fold(fixture, |fixture, (path, source)| {
            fixture.file(path, source)
        })
        .build(db)
}

fn symbols_for_source<'db>(db: &'db TestDatabase, source: &str) -> &'db TemplateSymbols {
    db.add_file("test.html", source);
    let file = db.file(Utf8Path::new("test.html"));
    let nodelist = parse_template(db, file).expect("should parse");
    template_symbols(db, nodelist)
}

fn inheritance_summary(
    db: &TestDatabase,
    project: djls_project::Project,
    file: File,
) -> (Vec<String>, ChainEnd) {
    let inheritance = template_inheritance(db, project, file);
    let ancestors = inheritance
        .ancestors(db)
        .iter()
        .map(|origin| origin.file(db).path(db).as_str().to_string())
        .collect();

    (ancestors, inheritance.end(db))
}

#[test]
fn extracts_partial_defs_from_partial_role_specs() {
    let mut specs = builtin_tag_specs();
    specs.merge(TagSpecs::new(FxHashMap::from_iter([(
        "partialdef".to_string(),
        TagSpec::new(
            Cow::Borrowed("django_template_partials.templatetags.partials"),
            Some(EndTag {
                name: Cow::Borrowed("endpartialdef"),
                required: true,
            }),
            Cow::Borrowed(&[]),
            false,
        )
        .with_role(TagRole::TemplatePartial),
    )])));
    let db = TestDatabase::new().with_specs(specs);
    let source = "{% partialdef card %}Body{% endpartialdef %}";
    let symbols = symbols_for_source(&db, source);

    assert!(symbols.blocks().is_empty());
    assert_eq!(
        symbols.partials(),
        &[PartialDef {
            name: "card".to_string(),
            name_span: Span::saturating_from_parts_usize(source.find("card").unwrap(), 4),
            full_span: Span::saturating_from_bounds_usize(0, source.len()),
        }]
    );
}

#[test]
fn unreadable_current_template_does_not_create_an_empty_tree() {
    let mut db = TestDatabase::new();
    let child_path = "/test/project/templates/child.html";
    let project = project_with_templates(
        &db,
        vec!["/test/project/templates"],
        vec![
            (child_path, "{% extends 'base.html' %}"),
            ("/test/project/templates/base.html", "base"),
        ],
    );
    let child = db.file(Utf8Path::new(child_path));
    assert_eq!(
        template_inheritance(&db, project, child)
            .ancestors(&db)
            .len(),
        1
    );

    db.remove_file(child_path);
    SourceChanges::new([ChangeEvent::Rescan]).apply(&mut db);

    let inheritance = template_inheritance(&db, project, child);
    assert!(inheritance.ancestors(&db).is_empty());
    assert_eq!(inheritance.end(&db), ChainEnd::Root);
}

#[test]
fn unreadable_ancestor_template_contributes_no_inherited_symbols() {
    let mut db = TestDatabase::new();
    let child_path = "/test/project/templates/child.html";
    let base_path = "/test/project/templates/base.html";
    let project = project_with_templates(
        &db,
        vec!["/test/project/templates"],
        vec![
            (child_path, "{% extends 'base.html' %}"),
            (base_path, "{% block title %}Base{% endblock %}"),
        ],
    );
    let child = db.file(Utf8Path::new(child_path));
    assert_eq!(inherited_blocks(&db, project, child).len(), 1);

    db.remove_file(base_path);
    SourceChanges::new([ChangeEvent::Rescan]).apply(&mut db);

    assert!(inherited_blocks(&db, project, child).is_empty());
}

#[test]
fn self_extends_skips_visited_origin_and_uses_shadowed_template() {
    let db = TestDatabase::new();
    let project = project_with_templates(
        &db,
        vec![
            "/test/project/templates",
            "/test/project/django_admin/templates",
        ],
        vec![
            (
                "/test/project/templates/admin/base_site.html",
                "{% extends \"admin/base_site.html\" %}\n{% block content %}Override{% endblock %}",
            ),
            (
                "/test/project/django_admin/templates/admin/base_site.html",
                "{% block content %}Default{% endblock %}",
            ),
        ],
    );
    let file = db.file(Utf8Path::new(
        "/test/project/templates/admin/base_site.html",
    ));

    let (ancestors, end) = inheritance_summary(&db, project, file);

    assert_eq!(
        ancestors,
        ["/test/project/django_admin/templates/admin/base_site.html"]
    );
    assert_eq!(end, ChainEnd::Root);
}

#[test]
fn template_inheritance_resolves_relative_sibling_extends() {
    let db = TestDatabase::new();
    let project = project_with_templates(
        &db,
        vec!["/test/project/templates"],
        vec![
            (
                "/test/project/templates/dir/child.html",
                "{% extends \"./parent.html\" %}",
            ),
            (
                "/test/project/templates/dir/parent.html",
                "{% block content %}Parent{% endblock %}",
            ),
        ],
    );
    let file = db.file(Utf8Path::new("/test/project/templates/dir/child.html"));

    let (ancestors, end) = inheritance_summary(&db, project, file);

    assert_eq!(ancestors, ["/test/project/templates/dir/parent.html"]);
    assert_eq!(end, ChainEnd::Root);
}

#[test]
fn template_inheritance_treats_escaping_relative_extends_as_unresolved() {
    let db = TestDatabase::new();
    let project = project_with_templates(
        &db,
        vec!["/test/project/templates"],
        vec![(
            "/test/project/templates/page.html",
            "{% extends \"../../outside.html\" %}",
        )],
    );
    let file = db.file(Utf8Path::new("/test/project/templates/page.html"));

    let (ancestors, end) = inheritance_summary(&db, project, file);

    assert!(ancestors.is_empty());
    assert_eq!(
        end,
        ChainEnd::Unresolved {
            name: "../../outside.html".to_string()
        }
    );
}

#[test]
fn block_overrides_accepts_relative_winning_extends_target() {
    let db = TestDatabase::new();
    let project = project_with_templates(
        &db,
        vec!["/test/project/templates"],
        vec![
            (
                "/test/project/templates/dir/parent.html",
                "{% block content %}Parent{% endblock %}",
            ),
            (
                "/test/project/templates/dir/child.html",
                "{% extends \"./parent.html\" %}\n{% block content %}Child{% endblock %}",
            ),
        ],
    );
    let parent = db.file(Utf8Path::new("/test/project/templates/dir/parent.html"));
    let child = db.file(Utf8Path::new("/test/project/templates/dir/child.html"));

    let overrides = block_overrides(&db, project, parent, "content");

    assert_eq!(overrides.len(), 1);
    assert!(overrides[0].file == child);
}

#[test]
fn template_inheritance_follows_extends_role_not_builtin_name() {
    let mut specs = builtin_tag_specs();
    specs.merge(TagSpecs::new(FxHashMap::from_iter([(
        "overextends".to_string(),
        TagSpec::new(
            Cow::Borrowed("myapp.templatetags.layout"),
            None,
            Cow::Borrowed(&[]),
            false,
        )
        .with_role(TagRole::TemplateReference(
            djls_semantic::TemplateReferenceKind::Extends,
        )),
    )])));
    let db = TestDatabase::new().with_specs(specs);
    let project = project_with_templates(
        &db,
        vec!["/test/project/templates"],
        vec![
            (
                "/test/project/templates/custom_child.html",
                "{% overextends \"base.html\" %}",
            ),
            (
                "/test/project/templates/builtin_child.html",
                "{% extends \"base.html\" %}",
            ),
            (
                "/test/project/templates/base.html",
                "{% block content %}Base{% endblock %}",
            ),
        ],
    );
    let custom_file = db.file(Utf8Path::new("/test/project/templates/custom_child.html"));
    let builtin_file = db.file(Utf8Path::new("/test/project/templates/builtin_child.html"));

    let custom = inheritance_summary(&db, project, custom_file);
    let builtin = inheritance_summary(&db, project, builtin_file);

    assert_eq!(custom, builtin);
}

#[test]
fn extracts_blocks_and_extends_by_role_not_builtin_names() {
    let mut specs = builtin_tag_specs();
    specs.merge(TagSpecs::new(FxHashMap::from_iter([
        (
            "section".to_string(),
            TagSpec::new(
                Cow::Borrowed("myapp.templatetags.layout"),
                Some(EndTag {
                    name: Cow::Borrowed("endsection"),
                    required: true,
                }),
                Cow::Borrowed(&[]),
                false,
            )
            .with_role(TagRole::TemplateBlock),
        ),
        (
            "overextends".to_string(),
            TagSpec::new(
                Cow::Borrowed("myapp.templatetags.layout"),
                None,
                Cow::Borrowed(&[]),
                false,
            )
            .with_role(TagRole::TemplateReference(
                djls_semantic::TemplateReferenceKind::Extends,
            )),
        ),
    ])));
    let db = TestDatabase::new().with_specs(specs);
    let source = r#"{% overextends "base.html" %}
{% section content %}Body{% endsection %}"#;
    let symbols = symbols_for_source(&db, source);

    assert_eq!(
        symbols.extends(),
        Some(&ExtendsTarget::Literal {
            name: "base.html".to_string(),
            span: Span::saturating_from_parts_usize(source.find("base.html").unwrap(), 9),
        })
    );
    assert_eq!(
        symbols.blocks(),
        &[BlockDef {
            name: "content".to_string(),
            name_span: Span::saturating_from_parts_usize(source.find("content").unwrap(), 7),
            full_span: Span::saturating_from_bounds_usize(
                source.find("{% section").unwrap(),
                source.len(),
            ),
        }]
    );
}

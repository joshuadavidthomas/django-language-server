use std::borrow::Cow;

use camino::Utf8Path;
use djls_project::Project;
use djls_semantic::BlockDef;
use djls_semantic::ChainEnd;
use djls_semantic::EndTag;
use djls_semantic::ExtendsTarget;
use djls_semantic::PartialDef;
use djls_semantic::TagRole;
use djls_semantic::TagSpec;
use djls_semantic::TagSpecs;
use djls_semantic::TemplateReferenceKind;
use djls_semantic::TemplateSymbols;
use djls_semantic::block_overrides;
use djls_semantic::builtin_tag_specs;
use djls_semantic::inherited_blocks;
use djls_semantic::parent_block;
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
) -> anyhow::Result<Project> {
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
        .file("/test/project/testproject/settings.py", settings_source)
        .file("/test/project/django/__init__.py", "")
        .file("/test/project/django/template/__init__.py", "")
        .file(
            "/test/project/django/template/defaulttags.py",
            "from django import template\nregister = template.Library()\n@register.tag\ndef load(parser, token): pass\n",
        )
        .file(
            "/test/project/django/template/loader_tags.py",
            "from django import template\nregister = template.Library()\n@register.tag\ndef block(parser, token): pass\n@register.tag\ndef extends(parser, token): pass\n@register.tag\ndef include(parser, token): pass\n",
        );

    templates
        .into_iter()
        .fold(fixture, |fixture, (path, source)| {
            fixture.file(path, source)
        })
        .build(db)
}

fn symbols_for_source<'db>(
    db: &'db TestDatabase,
    source: &str,
) -> anyhow::Result<&'db TemplateSymbols> {
    db.add_file("test.html", source)?;
    let file = db.file(Utf8Path::new("test.html"))?;
    let nodelist = match parse_template(db, file) {
        djls_templates::TemplateParseResult::Parsed(nodelist) => nodelist,
        djls_templates::TemplateParseResult::NotTemplate => {
            anyhow::bail!("fixture file is not a template")
        }
        djls_templates::TemplateParseResult::Unreadable(error) => return Err(error.into()),
    };
    Ok(template_symbols(db, file, nodelist))
}

fn inheritance_summary(db: &TestDatabase, project: Project, file: File) -> (Vec<String>, ChainEnd) {
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
    let db = TestDatabase::new().with_projectless_tag_specs(specs);
    let source = "{% partialdef card %}Body{% endpartialdef %}";
    let symbols = symbols_for_source(&db, source).expect("template symbol fixture should build");

    assert!(symbols.blocks().is_empty());
    assert_eq!(
        symbols.partials(),
        &[PartialDef {
            name: "card".to_string(),
            name_span: Span::saturating_from_parts_usize(
                source
                    .find("card")
                    .expect("fixture should contain the partial name"),
                4
            ),
            full_span: Span::saturating_from_bounds_usize(0, source.len()),
        }]
    );
}

#[test]
fn absent_effective_tag_does_not_fall_back_to_project_global_specs() {
    let mut specs = builtin_tag_specs();
    specs.insert(
        "overextends".to_string(),
        TagSpec::new(
            Cow::Borrowed("missing.templatetags.layout"),
            None,
            Cow::Borrowed(&[]),
            false,
        )
        .with_role(TagRole::TemplateReference(TemplateReferenceKind::Extends)),
    );
    let mut db = TestDatabase::new().with_projectless_tag_specs(specs);
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False}]\n",
        )
        .file(
            "/test/project/templates/child.html",
            "{% overextends 'base.html' %}",
        )
        .file("/test/project/templates/base.html", "base")
        .install(&mut db)
        .expect("project fixture should install into the test database");
    let child = db
        .file(Utf8Path::new("/test/project/templates/child.html"))
        .expect("fixture file should exist in the test database");

    assert_eq!(inheritance_summary(&db, project, child).1, ChainEnd::Root);
}

#[test]
fn inheritance_is_inconclusive_when_effective_extends_definition_conflicts_by_backend() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nif FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alpha_tags'}}}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'beta_tags'}}}]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/alpha_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag(name='extends')\ndef alpha_extends(value):\n    return value\n",
        )
        .file(
            "/test/project/beta_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag(name='extends')\ndef beta_extends():\n    return ''\n",
        )
        .file(
            "/test/project/templates/child.html",
            "{% load shared %}{% extends 'base.html' %}",
        )
        .file("/test/project/templates/base.html", "base")
        .build(&db)
        .expect("project fixture should build in the test database");
    db.set_project(project);
    let child = db
        .file(Utf8Path::new("/test/project/templates/child.html"))
        .expect("fixture file should exist in the test database");

    let (ancestors, end) = inheritance_summary(&db, project, child);
    assert!(ancestors.is_empty());
    assert_eq!(end, ChainEnd::Root);
}

#[test]
fn inheritance_keeps_child_backend_selection_when_resolving_parent() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/a'], 'APP_DIRS': False},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/b'], 'APP_DIRS': False},\n]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file("/test/project/django/__init__.py", "")
        .file("/test/project/django/template/__init__.py", "")
        .file(
            "/test/project/django/template/defaulttags.py",
            "from django import template\nregister = template.Library()\n@register.tag\ndef load(parser, token): pass\n",
        )
        .file(
            "/test/project/django/template/loader_tags.py",
            "from django import template\nregister = template.Library()\n@register.tag\ndef block(parser, token): pass\n@register.tag\ndef extends(parser, token): pass\n",
        )
        .file("/test/project/a/child.html", "{% extends 'base.html' %}")
        .file("/test/project/a/base.html", "backend a")
        .file("/test/project/b/base.html", "backend b")
        .build(&db)
        .expect("project fixture should build in the test database");
    db.set_project(project);
    let child = db
        .file(Utf8Path::new("/test/project/a/child.html"))
        .expect("fixture file should exist in the test database");

    let (ancestors, end) = inheritance_summary(&db, project, child);

    assert_eq!(ancestors, ["/test/project/a/base.html"]);
    assert_eq!(end, ChainEnd::Root);
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
    )
    .expect("template project fixture should build");
    let child = db
        .file(Utf8Path::new(child_path))
        .expect("fixture file should exist in the test database");
    assert_eq!(
        template_inheritance(&db, project, child)
            .ancestors(&db)
            .len(),
        1
    );

    db.remove_file(child_path)
        .expect("child template should be removed from the test database");
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
    )
    .expect("template project fixture should build");
    let child = db
        .file(Utf8Path::new(child_path))
        .expect("fixture file should exist in the test database");
    assert_eq!(inherited_blocks(&db, project, child).len(), 1);

    db.remove_file(base_path)
        .expect("base template should be removed from the test database");
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
    )
    .expect("template project fixture should build");
    let file = db
        .file(Utf8Path::new(
            "/test/project/templates/admin/base_site.html",
        ))
        .expect("admin base-site fixture should exist in the test database");

    let (ancestors, end) = inheritance_summary(&db, project, file);

    assert_eq!(
        ancestors,
        ["/test/project/django_admin/templates/admin/base_site.html"]
    );
    assert_eq!(end, ChainEnd::Root);
}

#[test]
fn originless_template_inheritance_resolves_absolute_extends_from_project_inventory() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False}]\n",
        )
        .file(
            "/test/project/scratch.html",
            "{% extends 'base.html' %}",
        )
        .file(
            "/test/project/templates/base.html",
            "{% block content %}Base{% endblock %}",
        )
        .install(&mut db)
        .expect("project fixture should install into the test database");
    let file = db
        .file(Utf8Path::new("/test/project/scratch.html"))
        .expect("fixture file should exist in the test database");

    assert_eq!(
        inheritance_summary(&db, project, file),
        (
            vec!["/test/project/templates/base.html".to_string()],
            ChainEnd::Root,
        )
    );
}

#[test]
fn originless_template_inheritance_preserves_project_resolution_alternatives() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nif FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/a'], 'APP_DIRS': False}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/b'], 'APP_DIRS': False}]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file("/test/project/scratch.html", "{% extends 'base.html' %}")
        .file("/test/project/a/base.html", "a")
        .file("/test/project/b/base.html", "b")
        .install(&mut db)
        .expect("project fixture should install into the test database");
    let file = db
        .file(Utf8Path::new("/test/project/scratch.html"))
        .expect("fixture file should exist in the test database");

    assert_eq!(
        inheritance_summary(&db, project, file),
        (
            Vec::new(),
            ChainEnd::InconclusiveParent {
                name: "base.html".to_string(),
            },
        )
    );
}

#[test]
fn originless_template_inheritance_leaves_relative_extends_unresolved() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False}]\n",
        )
        .file(
            "/test/project/scratch.html",
            "{% extends './base.html' %}",
        )
        .file("/test/project/templates/base.html", "base")
        .install(&mut db)
        .expect("project fixture should install into the test database");
    let file = db
        .file(Utf8Path::new("/test/project/scratch.html"))
        .expect("fixture file should exist in the test database");

    assert_eq!(
        inheritance_summary(&db, project, file),
        (
            Vec::new(),
            ChainEnd::Unresolved {
                name: "./base.html".to_string(),
            },
        )
    );
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
    )
    .expect("template project fixture should build");
    let file = db
        .file(Utf8Path::new("/test/project/templates/dir/child.html"))
        .expect("fixture file should exist in the test database");

    let (ancestors, end) = inheritance_summary(&db, project, file);

    assert_eq!(ancestors, ["/test/project/templates/dir/parent.html"]);
    assert_eq!(end, ChainEnd::Root);
}

#[test]
fn template_inheritance_joins_relative_targets_for_every_source_name() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates', '/test/project/templates/alias'], 'APP_DIRS': False}]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/templates/alias/child.html",
            "{% extends './parent.html' %}",
        )
        .file("/test/project/templates/alias/parent.html", "parent")
        .install(&mut db)
        .expect("project fixture should install into the test database");
    let child = db
        .file(Utf8Path::new("/test/project/templates/alias/child.html"))
        .expect("fixture file should exist in the test database");

    assert_eq!(
        inheritance_summary(&db, project, child),
        (
            vec!["/test/project/templates/alias/parent.html".to_string()],
            ChainEnd::Root,
        )
    );
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
    )
    .expect("template project fixture should build");
    let file = db
        .file(Utf8Path::new("/test/project/templates/page.html"))
        .expect("fixture file should exist in the test database");

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
    )
    .expect("template project fixture should build");
    let parent = db
        .file(Utf8Path::new("/test/project/templates/dir/parent.html"))
        .expect("fixture file should exist in the test database");
    let child = db
        .file(Utf8Path::new("/test/project/templates/dir/child.html"))
        .expect("fixture file should exist in the test database");

    let overrides = block_overrides(&db, project, parent, "content");

    assert_eq!(overrides.len(), 1);
    assert_eq!(overrides[0].file, child);
}

#[test]
fn reverse_inheritance_starts_from_secondary_names_and_dedupes_physical_sites() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates', '/test/project/templates/alias'], 'APP_DIRS': False}]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/templates/alias/base.html",
            "{% block content %}Base{% endblock %}",
        )
        .file(
            "/test/project/templates/alias/child.html",
            "{% extends './base.html' %}{% block content %}Child{% endblock %}",
        )
        .install(&mut db)
        .expect("project fixture should install into the test database");
    let base = db
        .file(Utf8Path::new("/test/project/templates/alias/base.html"))
        .expect("fixture file should exist in the test database");
    let child = db
        .file(Utf8Path::new("/test/project/templates/alias/child.html"))
        .expect("fixture file should exist in the test database");

    let overrides = block_overrides(&db, project, base, "content");

    assert_eq!(overrides.len(), 1);
    assert_eq!(overrides[0].file, child);
}

#[test]
fn originless_inheritance_keeps_the_exact_resolved_origin_for_relative_parents() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates', '/test/project/templates/alias'], 'APP_DIRS': False}]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/scratch.html",
            "{% extends 'alias/dir/parent.html' %}",
        )
        .file(
            "/test/project/templates/alias/dir/parent.html",
            "{% extends './base.html' %}",
        )
        .file("/test/project/templates/alias/dir/base.html", "exact base")
        .file("/test/project/templates/dir/base.html", "alias base")
        .install(&mut db)
        .expect("project fixture should install into the test database");
    let scratch = db
        .file(Utf8Path::new("/test/project/scratch.html"))
        .expect("fixture file should exist in the test database");

    assert_eq!(
        inheritance_summary(&db, project, scratch),
        (
            vec![
                "/test/project/templates/alias/dir/parent.html".to_string(),
                "/test/project/templates/alias/dir/base.html".to_string(),
            ],
            ChainEnd::Root,
        )
    );
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
        .with_role(TagRole::TemplateReference(TemplateReferenceKind::Extends)),
    )])));
    let db = TestDatabase::new().with_projectless_tag_specs(specs);
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
    )
    .expect("template project fixture should build");
    let custom_file = db
        .file(Utf8Path::new("/test/project/templates/custom_child.html"))
        .expect("fixture file should exist in the test database");
    let builtin_file = db
        .file(Utf8Path::new("/test/project/templates/builtin_child.html"))
        .expect("fixture file should exist in the test database");

    let custom = inheritance_summary(&db, project, custom_file);
    let builtin = inheritance_summary(&db, project, builtin_file);

    assert_eq!(custom, builtin);
}

#[test]
fn scoped_parent_miss_is_unresolved_when_name_exists_only_in_another_backend() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/a'], 'APP_DIRS': False},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/b'], 'APP_DIRS': False},\n]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file("/test/project/a/child.html", "{% extends 'other.html' %}")
        .file("/test/project/b/other.html", "other backend")
        .install(&mut db)
        .expect("project fixture should install into the test database");
    let child = db
        .file(Utf8Path::new("/test/project/a/child.html"))
        .expect("fixture file should exist in the test database");

    assert_eq!(
        inheritance_summary(&db, project, child),
        (
            Vec::new(),
            ChainEnd::Unresolved {
                name: "other.html".to_string(),
            },
        )
    );
}

#[test]
fn block_queries_stop_only_for_the_uncertain_name() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {**UNKNOWN}}}]\n",
        )
        .file(
            "/test/project/templates/base.html",
            "{% block title %}Base title{% endblock %}{% block sidebar %}Base sidebar{% endblock %}",
        )
        .file(
            "/test/project/templates/layout.html",
            "{% extends 'base.html' %}{% load unknown_library %}{% maybe_block sidebar %}",
        )
        .file(
            "/test/project/templates/child.html",
            "{% extends 'layout.html' %}{% block title %}Child title{% endblock %}{% block sidebar %}Child sidebar{% endblock %}",
        )
        .install(&mut db)
        .expect("uncertain Inheritance Block fixture should install");
    let child = db
        .file(Utf8Path::new("/test/project/templates/child.html"))
        .expect("child Template fixture should exist");
    let base = db
        .file(Utf8Path::new("/test/project/templates/base.html"))
        .expect("base Template fixture should exist");

    assert_eq!(
        parent_block(&db, project, child, "title").map(|site| site.file),
        Some(base),
        "unrelated uncertainty must not hide a definite parent block"
    );
    assert!(
        parent_block(&db, project, child, "sidebar").is_none(),
        "a possible nearer block must hide the grandparent block"
    );
    let inherited = inherited_blocks(&db, project, child);
    assert_eq!(
        inherited
            .iter()
            .map(|(name, site)| (name.as_str(), site.file))
            .collect::<Vec<_>>(),
        vec![("title", base)]
    );
}

#[test]
fn inherited_symbols_use_the_child_backend_scope() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared', '/test/project/a'], 'APP_DIRS': False},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared', '/test/project/b'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['custom_tags']}},\n]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/custom_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag(name='block')\ndef custom_block(value): pass\n",
        )
        .file("/test/project/a/child.html", "{% extends 'base.html' %}")
        .file(
            "/test/project/shared/base.html",
            "{% block content %}Base{% endblock %}",
        )
        .install(&mut db)
        .expect("project fixture should install into the test database");
    let child = db
        .file(Utf8Path::new("/test/project/a/child.html"))
        .expect("fixture file should exist in the test database");

    let inherited = inherited_blocks(&db, project, child);
    assert_eq!(inherited.len(), 1);
    assert_eq!(inherited[0].0, "content");
}

#[test]
fn reverse_inheritance_follows_the_exact_backend_origin() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/a'], 'APP_DIRS': False},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/b'], 'APP_DIRS': False},\n]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/a/base.html",
            "{% block content %}A{% endblock %}",
        )
        .file(
            "/test/project/b/base.html",
            "{% block content %}B{% endblock %}",
        )
        .file(
            "/test/project/b/child.html",
            "{% extends 'base.html' %}{% block content %}Child{% endblock %}",
        )
        .install(&mut db)
        .expect("project fixture should install into the test database");
    let a_base = db
        .file(Utf8Path::new("/test/project/a/base.html"))
        .expect("fixture file should exist in the test database");
    let b_base = db
        .file(Utf8Path::new("/test/project/b/base.html"))
        .expect("fixture file should exist in the test database");
    let child = db
        .file(Utf8Path::new("/test/project/b/child.html"))
        .expect("fixture file should exist in the test database");

    assert!(block_overrides(&db, project, a_base, "content").is_empty());
    assert_eq!(
        block_overrides(&db, project, b_base, "content")[0].file,
        child
    );
}

#[test]
fn reverse_inheritance_rejects_a_shared_child_with_backend_local_parents() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared', '/test/project/a'], 'APP_DIRS': False},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared', '/test/project/b'], 'APP_DIRS': False},\n]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/a/base.html",
            "{% block content %}A{% endblock %}",
        )
        .file(
            "/test/project/b/base.html",
            "{% block content %}B{% endblock %}",
        )
        .file(
            "/test/project/shared/child.html",
            "{% extends 'base.html' %}{% block content %}Child{% endblock %}",
        )
        .install(&mut db)
        .expect("shared-child backend fixture should install");
    let a_base = db
        .file(Utf8Path::new("/test/project/a/base.html"))
        .expect("backend A base Template should exist");
    let b_base = db
        .file(Utf8Path::new("/test/project/b/base.html"))
        .expect("backend B base Template should exist");

    assert!(block_overrides(&db, project, a_base, "content").is_empty());
    assert!(block_overrides(&db, project, b_base, "content").is_empty());
}

#[test]
fn template_inheritance_reports_missing_parent_as_unresolved() {
    let db = TestDatabase::new();
    let project = project_with_templates(
        &db,
        vec!["/test/project/templates"],
        vec![(
            "/test/project/templates/child.html",
            "{% extends 'missing.html' %}",
        )],
    )
    .expect("template project fixture should build");
    let child = db
        .file(Utf8Path::new("/test/project/templates/child.html"))
        .expect("fixture file should exist in the test database");

    let (ancestors, end) = inheritance_summary(&db, project, child);

    assert!(ancestors.is_empty());
    assert_eq!(
        end,
        ChainEnd::Unresolved {
            name: "missing.html".to_string()
        }
    );
}

#[test]
fn template_inheritance_reports_inconclusive_parent_search() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [UNKNOWN, '/test/project/templates'], 'APP_DIRS': False}]\n",
        )
        .file(
            "/test/project/templates/child.html",
            "{% extends 'base.html' %}",
        )
        .file("/test/project/templates/base.html", "base")
        .build(&db)
        .expect("project fixture should build in the test database");
    let child = db
        .file(Utf8Path::new("/test/project/templates/child.html"))
        .expect("fixture file should exist in the test database");

    let (ancestors, end) = inheritance_summary(&db, project, child);

    assert!(ancestors.is_empty());
    assert_eq!(
        end,
        ChainEnd::InconclusiveParent {
            name: "base.html".to_string()
        }
    );
}

#[test]
fn template_inheritance_preserves_resolved_prefix_before_inconclusive_parent() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates', UNKNOWN], 'APP_DIRS': False}]\n",
        )
        .file(
            "/test/project/templates/child.html",
            "{% extends 'parent.html' %}",
        )
        .file(
            "/test/project/templates/parent.html",
            "{% extends 'missing.html' %}",
        )
        .build(&db);
    assert!(project.is_ok(), "project fixture should build");
    let Some(project) = project.ok() else {
        return;
    };
    let child = db.file(Utf8Path::new("/test/project/templates/child.html"));
    assert!(child.is_ok(), "fixture file should exist");
    let Some(child) = child.ok() else {
        return;
    };

    assert_eq!(
        inheritance_summary(&db, project, child),
        (
            vec!["/test/project/templates/parent.html".to_string()],
            ChainEnd::InconclusiveParent {
                name: "missing.html".to_string(),
            },
        )
    );
}

#[test]
fn template_inheritance_preserves_cycle_detection() {
    let db = TestDatabase::new();
    let project = project_with_templates(
        &db,
        vec!["/test/project/templates"],
        vec![
            (
                "/test/project/templates/first.html",
                "{% extends 'second.html' %}",
            ),
            (
                "/test/project/templates/second.html",
                "{% extends 'first.html' %}",
            ),
        ],
    )
    .expect("template project fixture should build");
    let first = db
        .file(Utf8Path::new("/test/project/templates/first.html"))
        .expect("fixture file should exist in the test database");

    let (ancestors, end) = inheritance_summary(&db, project, first);

    assert_eq!(ancestors, ["/test/project/templates/second.html"]);
    assert_eq!(end, ChainEnd::Cycle);
}

#[test]
fn template_inheritance_detects_a_cycle_through_a_template_name_alias() {
    let db = TestDatabase::new();
    let project = project_with_templates(
        &db,
        vec!["/test/project/templates", "/test/project/templates/alias"],
        vec![
            (
                "/test/project/templates/start.html",
                "{% extends 'alias/b.html' %}",
            ),
            (
                "/test/project/templates/alias/b.html",
                "{% extends 'c.html' %}",
            ),
            (
                "/test/project/templates/alias/c.html",
                "{% extends 'b.html' %}",
            ),
        ],
    )
    .expect("template project fixture should build");
    let start = db
        .file(Utf8Path::new("/test/project/templates/start.html"))
        .expect("fixture file should exist in the test database");

    let (ancestors, end) = inheritance_summary(&db, project, start);

    assert_eq!(
        ancestors,
        [
            "/test/project/templates/alias/b.html",
            "/test/project/templates/alias/c.html",
        ]
    );
    assert_eq!(end, ChainEnd::Cycle);
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
            .with_role(TagRole::TemplateReference(TemplateReferenceKind::Extends)),
        ),
    ])));
    let db = TestDatabase::new().with_projectless_tag_specs(specs);
    let source = r#"{% overextends "base.html" %}
{% section content %}Body{% endsection %}"#;
    let symbols = symbols_for_source(&db, source).expect("template symbol fixture should build");

    assert_eq!(
        symbols.extends(),
        Some(&ExtendsTarget::Literal {
            name: "base.html".to_string(),
            span: Span::saturating_from_parts_usize(
                source
                    .find("base.html")
                    .expect("fixture should contain the parent template name"),
                9
            ),
        })
    );
    assert_eq!(
        symbols.blocks(),
        &[BlockDef {
            name: "content".to_string(),
            name_span: Span::saturating_from_parts_usize(
                source
                    .find("content")
                    .expect("fixture should contain the block name"),
                7
            ),
            full_span: Span::saturating_from_bounds_usize(
                source
                    .find("{% section")
                    .expect("fixture should contain the section opening tag"),
                source.len(),
            ),
        }]
    );
}

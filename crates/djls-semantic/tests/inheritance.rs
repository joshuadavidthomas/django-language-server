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

fn symbols_for_source<'db>(db: &'db TestDatabase, source: &str) -> &'db TemplateSymbols {
    db.add_file("test.html", source);
    let file = db.file(Utf8Path::new("test.html"));
    let nodelist = parse_template(db, file).expect("should parse");
    template_symbols(db, file, nodelist)
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
        .with_role(TagRole::TemplateReference(
            djls_semantic::TemplateReferenceKind::Extends,
        )),
    );
    let mut db = TestDatabase::new().with_specs(specs);
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
        .install(&mut db);
    let child = db.file(Utf8Path::new("/test/project/templates/child.html"));

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
        .build(&db);
    db.set_project(project);
    let child = db.file(Utf8Path::new("/test/project/templates/child.html"));

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
        .build(&db);
    db.set_project(project);
    let child = db.file(Utf8Path::new("/test/project/a/child.html"));

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
        .install(&mut db);
    let file = db.file(Utf8Path::new("/test/project/scratch.html"));

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
        .install(&mut db);
    let file = db.file(Utf8Path::new("/test/project/scratch.html"));

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
        .install(&mut db);
    let file = db.file(Utf8Path::new("/test/project/scratch.html"));

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
    );
    let file = db.file(Utf8Path::new("/test/project/templates/dir/child.html"));

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
        .install(&mut db);
    let child = db.file(Utf8Path::new("/test/project/templates/alias/child.html"));

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
        .install(&mut db);
    let base = db.file(Utf8Path::new("/test/project/templates/alias/base.html"));
    let child = db.file(Utf8Path::new("/test/project/templates/alias/child.html"));

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
        .install(&mut db);
    let scratch = db.file(Utf8Path::new("/test/project/scratch.html"));

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
fn scoped_parent_miss_is_unresolved_when_name_exists_only_in_another_backend() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/a'], 'APP_DIRS': False},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/b'], 'APP_DIRS': False},\n]\n";
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file("/test/project/a/child.html", "{% extends 'other.html' %}")
        .file("/test/project/b/other.html", "other backend")
        .install(&mut db);
    let child = db.file(Utf8Path::new("/test/project/a/child.html"));

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
        .install(&mut db);
    let child = db.file(Utf8Path::new("/test/project/a/child.html"));

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
        .install(&mut db);
    let a_base = db.file(Utf8Path::new("/test/project/a/base.html"));
    let b_base = db.file(Utf8Path::new("/test/project/b/base.html"));
    let child = db.file(Utf8Path::new("/test/project/b/child.html"));

    assert!(block_overrides(&db, project, a_base, "content").is_empty());
    assert_eq!(
        block_overrides(&db, project, b_base, "content")[0].file,
        child
    );
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
    );
    let child = db.file(Utf8Path::new("/test/project/templates/child.html"));

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
        .build(&db);
    let child = db.file(Utf8Path::new("/test/project/templates/child.html"));

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
    );
    let first = db.file(Utf8Path::new("/test/project/templates/first.html"));

    let (ancestors, end) = inheritance_summary(&db, project, first);

    assert_eq!(ancestors, ["/test/project/templates/second.html"]);
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

use std::borrow::Cow;
use std::io;

use camino::Utf8Path;
use djls_conf::TagDef;
use djls_conf::TagLibraryDef;
use djls_conf::TagSpecDef;
use djls_conf::TagTypeDef;
use djls_ide::completion;
use djls_project::ScopedTemplateLibraries;
use djls_project::SymbolDefinition;
use djls_project::TemplateSymbolKind;
use djls_project::template_library_catalog;
use djls_semantic::TagArgument;
use djls_semantic::TagArgumentKind;
use djls_semantic::TagSpec;
use djls_semantic::builtin_tag_specs;
use djls_source::Offset;
use djls_source::PositionEncoding;
use djls_testing::ProjectFixture;
use djls_testing::SalsaEventLog;
use djls_testing::TestDatabase;
use tower_lsp_server::ls_types;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

fn source_and_offset(marked_source: &str) -> TestResult<(String, Offset)> {
    let offset = marked_source
        .find('§')
        .ok_or_else(|| io::Error::other("test source should contain a cursor marker"))?;
    let mut source = marked_source.to_string();
    source.remove(offset);
    Ok((source, Offset::new(u32::try_from(offset)?)))
}

fn install_template_completion_project(
    db: &mut TestDatabase,
    child_path: &str,
    source: &str,
) -> TestResult<()> {
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates', '/test/project/app/templates'], 'APP_DIRS': False}]\n",
        )
        .file(child_path, source)
        .file("/test/project/templates/base.html", "base")
        .file("/test/project/templates/shared.html", "primary")
        .file("/test/project/app/templates/account/detail.html", "detail")
        .file("/test/project/app/templates/shared.html", "shadow")
        .install(db)?;
    Ok(())
}

#[test]
fn completion_dispatches_before_requesting_semantic_inventory() {
    let cases = [
        ("text", "plain §text", false, false),
        ("library", "{% load § %}", false, true),
        ("load-symbol", "{% load § from i18n %}", false, true),
        ("filter", "{{ value|§ }}", false, false),
        ("argument", "{% if § %}", false, true),
        ("tag-name", "{% § %}", true, true),
    ];

    for (name, marked_source, enumerates_tags, builds_projection) in cases {
        let event_log = SalsaEventLog::default();
        let db = TestDatabase::with_event_log(event_log.clone());
        let (source, offset) = source_and_offset(marked_source)
            .expect("completion case should contain a valid cursor marker");
        let path = format!("/{name}.html");
        db.add_file(&path, &source)
            .expect("completion fixture should be added");
        let file = db
            .file(Utf8Path::new(&path))
            .expect("completion fixture file should exist");
        event_log
            .take()
            .expect("initial Salsa events should be cleared");

        let response = completion(&db, file, offset, PositionEncoding::Utf16, false);
        let executed = event_log
            .take_will_execute_names(&db)
            .expect("completion Salsa events should be read");
        let executed_query = |query: &str| executed.iter().any(|name| name.ends_with(query));

        assert_eq!(
            executed_query("tag_specs_at"),
            enumerates_tags,
            "{name} completion ran unexpected tracked functions: {executed:?}"
        );
        assert_eq!(
            executed_query("template_analysis_projection_for_file_in_scope"),
            builds_projection,
            "{name} completion ran unexpected tracked functions: {executed:?}"
        );
        assert!(
            !executed_query("tag_specs_for_file"),
            "parsed completion contexts must not request the fallback tag inventory: {executed:?}"
        );
        if name == "text" {
            assert!(response.is_none());
            assert!(
                !executed_query("parse_template"),
                "text completion must stop before semantic parsing: {executed:?}"
            );
        }
        if name == "tag-name" {
            assert!(response.is_some());
        }
    }
}

#[test]
fn captured_closer_does_not_offer_colliding_standalone_arguments() {
    let mut specs = builtin_tag_specs();
    specs.insert(
        "endif".to_string(),
        TagSpec::new("test.tags".into(), None, Cow::Borrowed(&[]), false).with_arguments(vec![
            TagArgument {
                name: "collision".to_string(),
                kind: TagArgumentKind::Choice(vec!["standalone-choice".to_string()]),
                required: true,
                position: 0,
            },
        ]),
    );
    let db = TestDatabase::new().with_projectless_tag_specs(specs);

    let (captured_source, captured_offset) = source_and_offset("{% if condition %}{% endif § %}")
        .expect("captured closer fixture should contain a valid cursor marker");
    db.add_file("/captured.html", &captured_source)
        .expect("captured closer fixture should be added");
    assert!(
        completion(
            &db,
            db.file(Utf8Path::new("/captured.html"))
                .expect("captured closer fixture file should exist"),
            captured_offset,
            PositionEncoding::Utf16,
            false,
        )
        .is_none(),
        "a captured endif must not offer arguments from the colliding standalone definition"
    );

    let (standalone_source, standalone_offset) = source_and_offset("{% endif § %}")
        .expect("standalone closer fixture should contain a valid cursor marker");
    db.add_file("/standalone.html", &standalone_source)
        .expect("standalone closer fixture should be added");
    let response = completion(
        &db,
        db.file(Utf8Path::new("/standalone.html"))
            .expect("standalone closer fixture file should exist"),
        standalone_offset,
        PositionEncoding::Utf16,
        false,
    )
    .expect("the standalone endif definition should offer its argument");
    let items = match response {
        ls_types::CompletionResponse::Array(items) => items,
        ls_types::CompletionResponse::List(list) => list.items,
    };
    assert!(items.iter().any(|item| item.label == "standalone-choice"));
}

#[test]
fn shadowed_normal_tag_named_load_gets_no_library_completion() {
    let mut db = TestDatabase::new();
    let (library_source, library_offset) = source_and_offset("{% load § %}")
        .expect("library completion fixture should contain a valid cursor marker");
    let (symbol_source, symbol_offset) = source_and_offset("{% load custom_§ from custom %}")
        .expect("symbol completion fixture should contain a valid cursor marker");
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['custom_load'], 'libraries': {'custom': 'custom_tags'}}}]\n",
        )
        .file(
            "/test/project/custom_load.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag(name='load')\ndef custom_load(value): pass\n",
        )
        .file(
            "/test/project/custom_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef custom_tag(): pass\n",
        )
        .file("/test/project/templates/library.html", &library_source)
        .file("/test/project/templates/symbol.html", &symbol_source)
        .install(&mut db)
        .expect("shadowed-load project fixture should install");

    for (path, offset) in [
        ("/test/project/templates/library.html", library_offset),
        ("/test/project/templates/symbol.html", symbol_offset),
    ] {
        assert!(
            completion(
                &db,
                db.file(Utf8Path::new(path))
                    .expect("completion fixture file should exist"),
                offset,
                PositionEncoding::Utf16,
                false,
            )
            .is_none(),
            "a syntax-only load context must not bypass the point-resolved TagRole in {path}"
        );
    }
}

#[test]
fn project_tag_completions_do_not_leak_conflicting_backend_libraries() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/a'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alpha_tags'}}},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/b'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'beta_tags'}}},\n]\n";
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/alpha_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef alpha():\n    pass\n",
        )
        .file(
            "/test/project/beta_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef beta():\n    pass\n",
        )
        .file("/test/project/a/alpha.html", "{% load shared %}\n{%  %}")
        .file("/test/project/b/beta.html", "{% load shared %}\n{%  %}")
        .install(&mut db)
        .expect("multi-backend completion project fixture should install");

    let labels_for = |path: &str| -> TestResult<Vec<String>> {
        let file = db.file(Utf8Path::new(path))?;
        let response = completion(
            &db,
            file,
            Offset::new(
                u32::try_from("{% load shared %}\n{% ".len())
                    .expect("test source offset should fit in u32"),
            ),
            PositionEncoding::Utf16,
            false,
        )
        .ok_or_else(|| io::Error::other("tag completion should produce candidates"))?;
        Ok(match response {
            ls_types::CompletionResponse::Array(items) => items,
            ls_types::CompletionResponse::List(list) => list.items,
        }
        .into_iter()
        .map(|item| item.label)
        .collect())
    };

    let alpha = labels_for("/test/project/a/alpha.html")
        .expect("alpha backend completion labels should be collected");
    let beta = labels_for("/test/project/b/beta.html")
        .expect("beta backend completion labels should be collected");
    assert!(alpha.iter().any(|label| label == "alpha"));
    assert!(!alpha.iter().any(|label| label == "beta"));
    assert!(beta.iter().any(|label| label == "beta"));
    assert!(!beta.iter().any(|label| label == "alpha"));
}

#[test]
fn multi_backend_effective_symbols_disagree_when_implementations_differ() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nif FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alpha_tags'}}}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'beta_tags'}}}]\n";
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/alpha_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef common(): pass\n@register.filter(name='common_filter')\ndef alpha_filter(value): return value\n",
        )
        .file(
            "/test/project/beta_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef common(): pass\n@register.filter(name='common_filter')\ndef beta_filter(value): return value\n",
        )
        .file(
            "/test/project/shared/tag.html",
            "{% load shared %}\n{% com %}",
        )
        .file(
            "/test/project/shared/filter.html",
            "{% load shared %}\n{{ value|common_ }}",
        )
        .install(&mut db)
        .expect("conflicting-backend completion project fixture should install");

    let labels_at = |path: &str, source: &str| -> TestResult<Vec<String>> {
        let file = db.file(Utf8Path::new(path))?;
        let offset = Offset::new(
            u32::try_from(source.len()).expect("test source offset should fit in u32") - 3,
        );
        Ok(
            match completion(&db, file, offset, PositionEncoding::Utf16, false) {
                Some(ls_types::CompletionResponse::Array(items)) => items,
                Some(ls_types::CompletionResponse::List(list)) => list.items,
                None => Vec::new(),
            }
            .into_iter()
            .map(|item| item.label)
            .collect(),
        )
    };

    assert!(
        !labels_at(
            "/test/project/shared/tag.html",
            "{% load shared %}\n{% com %}",
        )
        .expect("tag completion labels should be collected")
        .contains(&"common".to_string())
    );
    assert!(
        !labels_at(
            "/test/project/shared/filter.html",
            "{% load shared %}\n{{ value|common_ }}",
        )
        .expect("filter completion labels should be collected")
        .contains(&"common_filter".to_string())
    );
}

#[test]
fn multi_backend_same_definition_uses_loaded_availability_presentation() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nif FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['shared_tags'], 'libraries': {'shared': 'empty_tags'}}}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'shared_tags'}}}]\n";
    let source = "{% load shared %}\n{% com %}";
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/shared_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef common(): pass\n",
        )
        .file(
            "/test/project/empty_tags.py",
            "from django import template\nregister = template.Library()\n",
        )
        .file("/test/project/shared/tag.html", source)
        .install(&mut db)
        .expect("shared-definition completion fixture should install");

    let file = db
        .file(Utf8Path::new("/test/project/shared/tag.html"))
        .expect("shared tag template fixture should exist");
    let offset =
        Offset::new(u32::try_from(source.len()).expect("test source offset should fit in u32") - 3);
    let response = completion(&db, file, offset, PositionEncoding::Utf16, false)
        .expect("the shared definition should complete");
    let items = match response {
        ls_types::CompletionResponse::Array(items) => items,
        ls_types::CompletionResponse::List(list) => list.items,
    };
    let item = items
        .into_iter()
        .find(|item| item.label == "common")
        .expect("the common tag should be offered");

    assert_eq!(item.detail.as_deref(), Some("{% load shared %}"));
}

#[test]
fn configured_only_tag_survives_effective_candidates_and_completion() {
    let mut db = TestDatabase::new();
    let (source, offset) = source_and_offset("{% load dynamic %}\n{% dynamic_§ %}")
        .expect("configured tag fixture should contain a valid cursor marker");
    let tag_specs = TagSpecDef {
        libraries: vec![TagLibraryDef {
            module: "dynamic_tags".to_string(),
            requires_engine: None,
            tags: vec![TagDef {
                name: "dynamic_panel".to_string(),
                tag_type: TagTypeDef::Standalone,
                end: None,
                intermediates: Vec::new(),
                args: Vec::new(),
                extra: None,
            }],
            extra: None,
        }],
        ..TagSpecDef::default()
    };
    let project = ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .tag_specs(tag_specs)
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nif FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['dynamic_tags'], 'libraries': {'dynamic': 'empty_tags'}}}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'dynamic': 'dynamic_tags'}}}]\n",
        )
        .file(
            "/test/project/dynamic_tags.py",
            "from django import template\nregister = template.Library()\nname = 'dynamic_panel'\nregister.tag(name, lambda parser, token: Node())\n",
        )
        .file(
            "/test/project/empty_tags.py",
            "from django import template\nregister = template.Library()\n",
        )
        .file("/test/project/templates/page.html", &source)
        .install(&mut db)
        .expect("configured-tag completion fixture should install");
    let file = db
        .file(Utf8Path::new("/test/project/templates/page.html"))
        .expect("configured-tag template fixture should exist");
    let configured_symbol =
        ScopedTemplateLibraries::from_project_inventory(template_library_catalog(&db, project))
            .resolved_libraries()
            .into_iter()
            .find(|library| library.module_name_str() == "dynamic_tags")
            .and_then(|library| library.symbol(TemplateSymbolKind::Tag, "dynamic_panel"))
            .expect("configured-only tag should enter its Template Library catalog");
    assert!(matches!(
        configured_symbol.definition,
        SymbolDefinition::Unknown
    ));

    let response = completion(&db, file, offset, PositionEncoding::Utf16, false)
        .expect("configured-only tag should complete");
    let items = match response {
        ls_types::CompletionResponse::Array(items) => items,
        ls_types::CompletionResponse::List(list) => list.items,
    };

    assert!(
        items.iter().any(|item| item.label == "dynamic_panel"),
        "configured-only Unknown definitions must agree by Template Library identity: {items:?}"
    );
}

#[test]
fn conflicting_backend_signatures_do_not_offer_argument_snippets() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nif FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alpha_tags'}}}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'beta_tags'}}}]\n";
    let marked = "{% load shared %}\n{% shared_tag § %}";
    let (source, offset) = source_and_offset(marked)
        .expect("conflicting signature fixture should contain a valid cursor marker");
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/alpha_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag(name='shared_tag')\ndef alpha(first):\n    pass\n",
        )
        .file(
            "/test/project/beta_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag(name='shared_tag')\ndef beta(first, second):\n    pass\n",
        )
        .file("/test/project/shared/page.html", &source)
        .install(&mut db)
        .expect("conflicting-signature project fixture should install");
    let file = db
        .file(Utf8Path::new("/test/project/shared/page.html"))
        .expect("shared template fixture should exist");

    assert!(
        completion(&db, file, offset, PositionEncoding::Utf16, true).is_none(),
        "disagreeing feasible signatures must not produce an argument snippet"
    );
}

#[test]
fn template_name_completions_do_not_leak_names_from_another_backend() {
    let mut db = TestDatabase::new();
    let (source, offset) = source_and_offset(r#"{% extends "§" %}"#)
        .expect("multi-backend template fixture should contain a valid cursor marker");
    let settings = "TEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/a'], 'APP_DIRS': False},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/b'], 'APP_DIRS': False},\n]\n";
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file("/test/project/a/child.html", &source)
        .file("/test/project/a/only-a.html", "a")
        .file("/test/project/b/only-b.html", "b")
        .install(&mut db)
        .expect("multi-backend template completion fixture should install");
    let file = db
        .file(Utf8Path::new("/test/project/a/child.html"))
        .expect("child template fixture should exist");

    let response = completion(&db, file, offset, PositionEncoding::Utf16, false)
        .expect("template names should complete");
    let labels = match response {
        ls_types::CompletionResponse::Array(items) => items,
        ls_types::CompletionResponse::List(list) => list.items,
    }
    .into_iter()
    .map(|item| item.label)
    .collect::<Vec<_>>();
    assert!(labels.contains(&"only-a.html".to_string()));
    assert!(!labels.contains(&"only-b.html".to_string()));
}

#[test]
fn template_name_completions_use_resolvable_project_names() {
    let mut db = TestDatabase::new();
    let (source, offset) = source_and_offset(r#"{% extends "§" %}"#)
        .expect("template name fixture should contain a valid cursor marker");
    let child_path = "/test/project/templates/child.html";
    install_template_completion_project(&mut db, child_path, &source)
        .expect("template completion project fixture should install");
    let file = db
        .file(Utf8Path::new(child_path))
        .expect("child template fixture should exist");

    let response = completion(&db, file, offset, PositionEncoding::Utf16, false)
        .expect("template names should complete inside quoted references");
    let items = match response {
        ls_types::CompletionResponse::Array(items) => items,
        ls_types::CompletionResponse::List(list) => list.items,
    };
    let labels = items
        .iter()
        .map(|item| item.label.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        labels,
        vec![
            "account/detail.html",
            "base.html",
            "child.html",
            "shared.html",
        ]
    );
    assert_eq!(items[0].kind, Some(ls_types::CompletionItemKind::FILE));
    assert_eq!(items[0].detail.as_deref(), Some("Django template"));
}

#[test]
fn template_name_completions_retain_known_templates_when_search_is_incomplete() {
    let mut db = TestDatabase::new();
    let (source, offset) = source_and_offset(r#"{% extends "§" %}"#)
        .expect("incomplete-search fixture should contain a valid cursor marker");
    let child_path = "/test/project/templates/child.html";
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [UNKNOWN, '/test/project/templates'], 'APP_DIRS': False}]\n",
        )
        .file(child_path, &source)
        .file("/test/project/templates/base.html", "base")
        .install(&mut db)
        .expect("incomplete-search completion fixture should install");
    let file = db
        .file(Utf8Path::new(child_path))
        .expect("child template fixture should exist");

    let response = completion(&db, file, offset, PositionEncoding::Utf16, false)
        .expect("known template names should remain completion candidates");
    let items = match response {
        ls_types::CompletionResponse::Array(items) => items,
        ls_types::CompletionResponse::List(list) => list.items,
    };
    let labels = items
        .iter()
        .map(|item| item.label.as_str())
        .collect::<Vec<_>>();

    assert_eq!(labels, ["base.html", "child.html"]);
}

#[test]
fn template_name_completion_replaces_quoted_argument_interior() {
    let mut db = TestDatabase::new();
    let (source, offset) = source_and_offset(r#"{% extends "acc§ount/detail.html" %}"#)
        .expect("quoted template fixture should contain a valid cursor marker");
    let child_path = "/test/project/templates/child.html";
    install_template_completion_project(&mut db, child_path, &source)
        .expect("template completion project fixture should install");
    let file = db
        .file(Utf8Path::new(child_path))
        .expect("child template fixture should exist");

    let response = completion(&db, file, offset, PositionEncoding::Utf16, false)
        .expect("template names should complete inside quoted references");
    let items = match response {
        ls_types::CompletionResponse::Array(items) => items,
        ls_types::CompletionResponse::List(list) => list.items,
    };
    let item = items
        .iter()
        .find(|item| item.label == "account/detail.html")
        .expect("expected account/detail.html completion");
    let text_edit = item
        .text_edit
        .as_ref()
        .expect("template-name completion should carry a text edit");

    assert_eq!(
        text_edit,
        &ls_types::CompletionTextEdit::Edit(ls_types::TextEdit::new(
            ls_types::Range::new(
                ls_types::Position::new(0, 12),
                ls_types::Position::new(0, 31),
            ),
            "account/detail.html".to_string(),
        ))
    );
}

#[test]
fn template_name_completion_preserves_existing_full_close_after_open_quote() {
    let mut db = TestDatabase::new();
    let (source, offset) = source_and_offset(r#"{% extends "ba§ %}"#)
        .expect("open quote fixture should contain a valid cursor marker");
    let child_path = "/test/project/templates/child.html";
    install_template_completion_project(&mut db, child_path, &source)
        .expect("template completion project fixture should install");
    let file = db
        .file(Utf8Path::new(child_path))
        .expect("child template fixture should exist");

    let response = completion(&db, file, offset, PositionEncoding::Utf16, false)
        .expect("template names should complete inside quoted references");
    let items = match response {
        ls_types::CompletionResponse::Array(items) => items,
        ls_types::CompletionResponse::List(list) => list.items,
    };
    let item = items
        .iter()
        .find(|item| item.label == "base.html")
        .expect("expected base.html completion");
    let text_edit = item
        .text_edit
        .as_ref()
        .expect("template-name completion should carry a text edit");

    assert_eq!(
        text_edit,
        &ls_types::CompletionTextEdit::Edit(ls_types::TextEdit::new(
            ls_types::Range::new(
                ls_types::Position::new(0, 12),
                ls_types::Position::new(0, 14),
            ),
            "base.html\"".to_string(),
        ))
    );
}

#[test]
fn template_name_completion_repairs_autopaired_quote_before_lone_brace() {
    let mut db = TestDatabase::new();
    let (source, offset) = source_and_offset(r#"{% include "ba§"}"#)
        .expect("autopaired quote fixture should contain a valid cursor marker");
    let child_path = "/test/project/templates/child.html";
    install_template_completion_project(&mut db, child_path, &source)
        .expect("template completion project fixture should install");
    let file = db
        .file(Utf8Path::new(child_path))
        .expect("child template fixture should exist");

    let response = completion(&db, file, offset, PositionEncoding::Utf16, false)
        .expect("template names should complete inside quoted references");
    let items = match response {
        ls_types::CompletionResponse::Array(items) => items,
        ls_types::CompletionResponse::List(list) => list.items,
    };
    let item = items
        .iter()
        .find(|item| item.label == "base.html")
        .expect("expected base.html completion");
    let text_edit = item
        .text_edit
        .as_ref()
        .expect("template-name completion should carry a text edit");

    assert_eq!(
        text_edit,
        &ls_types::CompletionTextEdit::Edit(ls_types::TextEdit::new(
            ls_types::Range::new(
                ls_types::Position::new(0, 12),
                ls_types::Position::new(0, 16),
            ),
            "base.html\" %}".to_string(),
        ))
    );
}

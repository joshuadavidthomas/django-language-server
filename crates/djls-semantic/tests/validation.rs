use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt::Write;
use std::fs;
use std::path::PathBuf;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::TagDef;
use djls_conf::TagLibraryDef;
use djls_conf::TagSpecDef;
use djls_conf::TagTypeDef;
use djls_project::ArgumentCountConstraint;
use djls_project::Project;
use djls_project::ScopedTemplateLibraries;
use djls_project::SymbolDefinition;
use djls_project::TagRule;
use djls_project::TemplateSymbolKind;
use djls_project::UnreadRegistration;
use djls_project::UnreadShape;
use djls_project::template_library_catalog;
use djls_semantic::Db as SemanticDb;
use djls_semantic::TagArgumentKind;
use djls_semantic::TagArgumentSyntax;
use djls_semantic::TagRole;
use djls_semantic::TagSpec;
use djls_semantic::TagSpecs;
use djls_semantic::ValidationError;
use djls_semantic::builtin_tag_specs;
use djls_semantic::library_tag_specs;
use djls_semantic::semantic_grammar_vocabulary;
use djls_semantic::tag_spec_at;
use djls_semantic::tag_specs_for_file;
use djls_templates::parse_template;
use djls_testing::OsTestDatabase;
use djls_testing::ProjectFixture;
use djls_testing::ProjectSettings;
use djls_testing::TestDatabase;
use djls_testing::collect_errors as collect_validation_errors;
use djls_testing::corpus_project_database;

fn configured_tag_specs(definitions: &[(&str, &str, TagTypeDef)]) -> TagSpecDef {
    TagSpecDef {
        libraries: definitions
            .iter()
            .map(|(module, name, tag_type)| TagLibraryDef {
                module: (*module).to_string(),
                requires_engine: None,
                tags: vec![TagDef {
                    name: (*name).to_string(),
                    tag_type: tag_type.clone(),
                    end: None,
                    intermediates: Vec::new(),
                    args: Vec::new(),
                    extra: None,
                }],
                extra: None,
            })
            .collect(),
        ..TagSpecDef::default()
    }
}

fn collect_test_errors(db: &TestDatabase, source: &str) -> anyhow::Result<Vec<ValidationError>> {
    collect_errors(db, "test.html", source)
}

fn collect_errors(
    db: &TestDatabase,
    path: &str,
    source: &str,
) -> anyhow::Result<Vec<ValidationError>> {
    db.add_file(path, source)?;
    let file = db.create_file_with_revision(Utf8Path::new(path), 0)?;
    Ok(collect_validation_errors(db, file))
}

fn collect_file_errors(
    db: &dyn djls_semantic::Db,
    path: &str,
) -> anyhow::Result<Vec<ValidationError>> {
    let file = djls_source::path_to_file(db, Utf8Path::new(path))?;
    Ok(collect_validation_errors(db, file))
}

fn validation_project_database(
    name: &str,
) -> anyhow::Result<(OsTestDatabase, Project, Utf8PathBuf)> {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resources/projects")
        .join(name)
        .canonicalize()
        .map_err(|error| anyhow::anyhow!("failed to resolve project fixture `{name}`: {error}"))?;
    let project_root = Utf8PathBuf::from_path_buf(project_root)
        .map_err(|path| anyhow::anyhow!("fixture path should be UTF-8: {}", path.display()))?;
    let (db, project, _) =
        corpus_project_database(project_root.clone(), [project_root.clone()], "settings")?;
    Ok((db, project, project_root))
}

#[test]
fn repeated_project_symbols_keep_occurrence_diagnostics_across_load_prefixes() {
    let mut db = TestDatabase::new();
    let source = concat!(
        "{% load extras %}\n",
        "{% custom %}\n",
        "{% custom %}\n",
        "{{ first|needs_arg }}\n",
        "{{ second|needs_arg }}\n",
        "{% load extras %}\n",
        "{% custom %}\n",
        "{% custom %}\n",
        "{{ third|needs_arg }}\n",
        "{{ fourth|needs_arg }}\n",
    );
    ProjectFixture::new("/proj")
        .tag_specs(configured_tag_specs(&[(
            "custom_tags",
            "custom",
            TagTypeDef::Standalone,
        )]))
        .settings(&ProjectSettings {
            dirs: vec!["/proj/templates".to_string()],
            libraries: BTreeMap::from([("extras".to_string(), "custom_tags".to_string())]),
            ..ProjectSettings::default()
        })
        .file(
            "/proj/custom_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef custom(value):\n    pass\n@register.filter\ndef needs_arg(value, arg):\n    return value\n",
        )
        .file("/proj/templates/page.html", source)
        .install(&mut db)
        .expect("project fixture should install into the test database");

    let errors = collect_file_errors(&db, "/proj/templates/page.html")
        .expect("fixture file validation errors should be collected");
    let expected_tag_starts = source
        .match_indices("{% custom")
        .map(|(start, _)| u32::try_from(start).expect("fixture offset should fit in u32"))
        .collect::<Vec<_>>();
    let expected_filter_starts = source
        .match_indices("needs_arg")
        .map(|(start, _)| u32::try_from(start).expect("fixture offset should fit in u32"))
        .collect::<Vec<_>>();
    let tag_argument_starts = errors
        .iter()
        .filter_map(|error| {
            if let ValidationError::ExtractedRuleViolation { tag, span, .. } = error
                && tag == "custom"
            {
                return Some(span.start());
            }
            None
        })
        .collect::<Vec<_>>();
    let filter_arity_starts = errors
        .iter()
        .filter_map(|error| {
            if let ValidationError::FilterMissingArgument { filter, span } = error
                && filter == "needs_arg"
            {
                return Some(span.start());
            }
            None
        })
        .collect::<Vec<_>>();

    assert_eq!(
        tag_argument_starts, expected_tag_starts,
        "repeated tags should retain their own argument diagnostics: {errors:?}"
    );
    assert_eq!(
        filter_arity_starts, expected_filter_starts,
        "repeated filters should retain their own arity diagnostics: {errors:?}"
    );
    assert!(
        !errors.iter().any(|error| matches!(
            error,
            ValidationError::UnknownTag { .. }
                | ValidationError::UnloadedTag { .. }
                | ValidationError::UnknownFilter { .. }
                | ValidationError::UnloadedFilter { .. }
        )),
        "both repeated load prefixes should make the symbols available: {errors:?}"
    );
}

#[test]
fn source_less_configured_library_preserves_block_structure() {
    let mut db = TestDatabase::new();
    ProjectFixture::new("/proj")
        .tag_specs(configured_tag_specs(&[(
            "missing.panel_tags",
            "panel",
            TagTypeDef::Block,
        )]))
        .settings(&ProjectSettings {
            dirs: vec!["/proj/templates".to_string()],
            libraries: BTreeMap::from([("panels".to_string(), "missing.panel_tags".to_string())]),
            ..ProjectSettings::default()
        })
        .file(
            "/proj/templates/page.html",
            "{% load panels %}{% panel %}body",
        )
        .install(&mut db)
        .expect("project fixture should install into the test database");

    let errors = collect_file_errors(&db, "/proj/templates/page.html")
        .expect("fixture file validation errors should be collected");
    assert!(
        errors.iter().any(
            |error| matches!(error, ValidationError::UnclosedTag { tag, .. } if tag == "panel")
        ),
        "configured structure should remain active without Python source: {errors:?}"
    );
    assert!(!errors.iter().any(|error| matches!(
        error,
        ValidationError::UnknownLibrary { name, .. }
            | ValidationError::UnknownTag { tag: name, .. }
            | ValidationError::UnloadedTag { tag: name, .. }
            if name == "panels" || name == "panel"
    )));
}

#[test]
fn source_less_default_builtins_keep_django_grammar_and_load_configured_library() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .tag_specs(configured_tag_specs(&[(
            "missing.panel_tags",
            "panel",
            TagTypeDef::Standalone,
        )]))
        .settings(&ProjectSettings {
            dirs: vec!["/proj/templates".to_string()],
            libraries: BTreeMap::from([("panels".to_string(), "missing.panel_tags".to_string())]),
            ..ProjectSettings::default()
        })
        .file(
            "/proj/templates/page.html",
            "{% load panels %}{% panel %}{% if condition %}{% for item in items %}{% comment %}{% endfor %}{% endif %}{% endcomment %}{% empty %}empty{% endfor %}{% else %}fallback{% endif %}",
        )
        .build(&db)
        .expect("project fixture should build in the test database");
    db.set_project(project);

    let libraries = template_library_catalog(&db, project);
    let scoped_libraries = ScopedTemplateLibraries::from_project_inventory(libraries);
    for module in [
        "django.template.defaulttags",
        "django.template.defaultfilters",
        "django.template.loader_tags",
    ] {
        let library = scoped_libraries
            .resolved_libraries()
            .into_iter()
            .find(|library| library.module_name_str() == module)
            .expect("canonical default builtin identity should remain present");
        assert!(library.source_file().is_none());
        assert!(library.symbols_are_unobserved());
    }
    let panel_library = scoped_libraries
        .resolved_libraries()
        .into_iter()
        .find(|library| library.module_name_str() == "missing.panel_tags")
        .expect("configured source-less library should remain present");
    assert!(panel_library.source_file().is_none());

    let defaulttags = scoped_libraries
        .resolved_libraries()
        .into_iter()
        .find(|library| library.module_name_str() == "django.template.defaulttags")
        .expect("defaulttags identity should remain present");
    let specs = library_tag_specs(&db, project, defaulttags.id());
    for name in ["if", "for", "load", "comment", "verbatim"] {
        assert!(
            specs.get(name).is_some(),
            "missing hardcoded spec for {name}"
        );
    }

    let errors = collect_file_errors(&db, "/proj/templates/page.html")
        .expect("fixture file validation errors should be collected");
    assert!(
        !errors.iter().any(|error| matches!(
            error,
            ValidationError::UnknownLibrary { name, .. }
                | ValidationError::UnknownTag { tag: name, .. }
                | ValidationError::UnloadedTag { tag: name, .. }
                | ValidationError::UnclosedTag { tag: name, .. }
                | ValidationError::OrphanedTag { tag: name, .. }
                if matches!(
                    name.as_str(),
                    "panels"
                        | "panel"
                        | "if"
                        | "for"
                        | "comment"
                        | "endcomment"
                        | "empty"
                        | "else"
                        | "endfor"
                        | "endif"
                )
        )),
        "source-less canonical grammar and configured load should stay effective: {errors:?}"
    );
}

#[test]
fn configured_same_name_specs_remain_keyed_by_library() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .tag_specs(configured_tag_specs(&[
            ("alpha_tags", "shared", TagTypeDef::Block),
            ("beta_tags", "shared", TagTypeDef::Standalone),
        ]))
        .settings(&ProjectSettings {
            dirs: vec!["/proj/templates".to_string()],
            libraries: BTreeMap::from([
                ("alpha".to_string(), "alpha_tags".to_string()),
                ("beta".to_string(), "beta_tags".to_string()),
            ]),
            ..ProjectSettings::default()
        })
        .file(
            "/proj/alpha_tags.py",
            "from django import template\nregister = template.Library()\n",
        )
        .file(
            "/proj/beta_tags.py",
            "from django import template\nregister = template.Library()\n",
        )
        .file("/proj/templates/page.html", "")
        .install(&mut db)
        .expect("project fixture should install into the test database");

    let libraries = template_library_catalog(&db, project);
    let scoped_libraries = ScopedTemplateLibraries::from_project_inventory(libraries);
    let alpha = scoped_libraries
        .resolved_libraries()
        .into_iter()
        .find(|library| library.module_name_str() == "alpha_tags")
        .expect("alpha should resolve");
    let beta = scoped_libraries
        .resolved_libraries()
        .into_iter()
        .find(|library| library.module_name_str() == "beta_tags")
        .expect("beta should resolve");

    assert!(
        library_tag_specs(&db, project, alpha.id())
            .get("shared")
            .and_then(|spec| spec.end_tag.as_ref())
            .is_some(),
        "alpha's configured block shape must not be overwritten by beta"
    );
    assert!(
        library_tag_specs(&db, project, beta.id())
            .get("shared")
            .is_some_and(|spec| spec.end_tag.is_none()),
        "beta's configured standalone shape must not inherit alpha's same-name spec"
    );
}

#[test]
fn configured_source_registration_is_available_through_its_library_catalog() {
    let mut db = TestDatabase::new();
    let project = ProjectFixture::new("/proj")
        .tag_specs(configured_tag_specs(&[(
            "dynamic_tags",
            "dynamic_panel",
            TagTypeDef::Block,
        )]))
        .settings(&ProjectSettings {
            dirs: vec!["/proj/templates".to_string()],
            libraries: BTreeMap::from([("dynamic".to_string(), "dynamic_tags".to_string())]),
            ..ProjectSettings::default()
        })
        .file(
            "/proj/dynamic_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef sourced_tag():\n    return ''\ndef compile_panel(parser, token):\n    return Node()\ntag_name = 'dynamic_panel'\nregister.tag(tag_name, compile_panel)\n",
        )
        .file(
            "/proj/templates/page.html",
            "{% load dynamic %}{% dynamic_panel %}body{% enddynamic_panel %}",
        )
        .install(&mut db)
        .expect("project fixture should install into the test database");

    let libraries = template_library_catalog(&db, project);
    let dynamic = ScopedTemplateLibraries::from_project_inventory(libraries)
        .resolved_libraries()
        .into_iter()
        .find(|library| library.module_name_str() == "dynamic_tags")
        .expect("configured dynamic library should resolve");
    let symbol = dynamic
        .symbol(TemplateSymbolKind::Tag, "dynamic_panel")
        .expect("configured-only definition should enter the keyed catalog");
    assert!(matches!(symbol.definition, SymbolDefinition::Exact { .. }));
    assert!(matches!(
        dynamic
            .symbol(TemplateSymbolKind::Tag, "sourced_tag")
            .expect("source registration should remain cataloged")
            .definition,
        SymbolDefinition::Exact { .. }
    ));
    assert!(
        library_tag_specs(&db, project, dynamic.id())
            .get("dynamic_panel")
            .is_some(),
        "configured-only definition should enter the keyed semantic product"
    );

    let errors = collect_file_errors(&db, "/proj/templates/page.html")
        .expect("fixture file validation errors should be collected");
    assert!(
        !errors.iter().any(|error| matches!(
            error,
            ValidationError::UnknownTag { tag, .. }
                | ValidationError::UnloadedTag { tag, .. }
                | ValidationError::UnclosedTag { tag, .. }
                | ValidationError::OrphanedTag { tag, .. }
                if tag == "dynamic_panel" || tag == "enddynamic_panel"
        )),
        "configured dynamic registration should have loaded block meaning: {errors:?}"
    );
}

#[test]
fn configured_arguments_fill_a_kwargs_only_simple_tag() {
    let mut db = TestDatabase::new();
    let tag_specs: TagSpecDef = serde_json::from_value(serde_json::json!({
        "libraries": [{
            "module": "dynamic_tags",
            "tags": [{
                "name": "configured",
                "type": "standalone",
                "args": [{
                    "name": "mode",
                    "kind": "choice",
                    "extra": {"choices": ["small", "large"]}
                }]
            }]
        }]
    }))
    .expect("configured argument fixture should deserialize");
    let project = ProjectFixture::new("/proj")
        .tag_specs(tag_specs)
        .settings(&ProjectSettings {
            dirs: vec!["/proj/templates".to_string()],
            libraries: BTreeMap::from([("dynamic".to_string(), "dynamic_tags".to_string())]),
            ..ProjectSettings::default()
        })
        .file(
            "/proj/dynamic_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef configured(**kwargs):\n    return ''\n",
        )
        .file(
            "/proj/templates/page.html",
            "{% load dynamic %}{% configured small as selected %}",
        )
        .install(&mut db)
        .expect("configured kwargs project fixture should install");

    let library =
        ScopedTemplateLibraries::from_project_inventory(template_library_catalog(&db, project))
            .resolved_libraries()
            .into_iter()
            .find(|library| library.module_name_str() == "dynamic_tags")
            .expect("configured Template Library should resolve");
    let spec = library_tag_specs(&db, project, library.id())
        .get("configured")
        .cloned()
        .expect("configured tag should have an effective spec");
    let parameters = spec
        .argument_syntax()
        .parameters()
        .expect("configured arguments should replace empty signature evidence");

    assert!(matches!(
        spec.argument_syntax(),
        TagArgumentSyntax::Parameters(_)
    ));
    assert_eq!(parameters.len(), 1);
    assert_eq!(
        parameters[0].kind,
        TagArgumentKind::Choice(vec!["small".to_string(), "large".to_string()])
    );
    assert!(
        collect_file_errors(&db, "/proj/templates/page.html")
            .expect("configured fallback template should validate")
            .is_empty()
    );
}

#[test]
fn configured_fallback_does_not_weaken_empty_trusted_signatures() {
    let mut db = TestDatabase::new();
    let tag_specs: TagSpecDef = serde_json::from_value(serde_json::json!({
        "libraries": [{
            "module": "strict_tags",
            "tags": [
                {
                    "name": "empty_helper",
                    "type": "standalone",
                    "args": [{"name": "invented", "kind": "variable"}]
                },
                {
                    "name": "kwargs_helper",
                    "type": "standalone",
                    "args": []
                }
            ]
        }]
    }))
    .expect("strict configured fallback fixture should deserialize");
    let project = ProjectFixture::new("/proj")
        .tag_specs(tag_specs)
        .settings(&ProjectSettings {
            dirs: vec!["/proj/templates".to_string()],
            libraries: BTreeMap::from([("strict".to_string(), "strict_tags".to_string())]),
            ..ProjectSettings::default()
        })
        .file(
            "/proj/strict_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef empty_helper(): return ''\n@register.simple_tag\ndef kwargs_helper(**options): return options\n",
        )
        .file(
            "/proj/templates/page.html",
            "{% load strict %}{% empty_helper invented %}{% kwargs_helper positional %}",
        )
        .install(&mut db)
        .expect("strict configured fallback project should install");

    let library =
        ScopedTemplateLibraries::from_project_inventory(template_library_catalog(&db, project))
            .resolved_libraries()
            .into_iter()
            .find(|library| library.module_name_str() == "strict_tags")
            .expect("strict Template Library should resolve");
    let specs = library_tag_specs(&db, project, library.id());
    assert!(matches!(
        specs
            .get("empty_helper")
            .expect("empty helper should have a spec")
            .argument_syntax(),
        TagArgumentSyntax::Signature {
            parameters,
            variadic_keyword: None,
            ..
        } if parameters.is_empty()
    ));
    assert!(matches!(
        specs
            .get("kwargs_helper")
            .expect("kwargs helper should have a spec")
            .argument_syntax(),
        TagArgumentSyntax::Signature {
            parameters,
            variadic_keyword: Some(name),
            ..
        } if parameters.is_empty() && name == "options"
    ));

    let errors = collect_file_errors(&db, "/proj/templates/page.html")
        .expect("strict configured fallback template should validate");
    assert!(errors.iter().any(|error| matches!(
        error,
        ValidationError::ExtractedRuleViolation { tag, message, .. }
            if tag == "empty_helper" && message.contains("too many positional")
    )));
    assert!(errors.iter().any(|error| matches!(
        error,
        ValidationError::ExtractedRuleViolation { tag, message, .. }
            if tag == "kwargs_helper" && message.contains("too many positional")
    )));
}

#[test]
fn loaded_imported_signature_rebinds_after_source_invalidation() {
    let (mut db, project, project_root) = validation_project_database("rebinding-import")
        .expect("loaded imported signature fixture should install");
    let template_path = project_root.join("templates/page.html");

    let library =
        ScopedTemplateLibraries::from_project_inventory(template_library_catalog(&db, project))
            .resolved_libraries()
            .into_iter()
            .find(|library| library.module_name_str() == "app.templatetags.authored")
            .expect("authored Template Library should resolve");
    let before = library_tag_specs(&db, project, library.id())
        .get("loaded_imported")
        .cloned()
        .expect("imported tag should have a semantic spec");
    assert!(matches!(
        before.argument_syntax(),
        TagArgumentSyntax::Signature { parameters, variadic_keyword: None, .. }
            if parameters.len() == 1
                && parameters[0].name == "value"
                && parameters[0].requirement.is_required()
    ));
    assert!(
        collect_file_errors(&db, template_path.as_str())
            .expect("missing imported argument should validate")
            .iter()
            .any(|error| matches!(
                error,
                ValidationError::ExtractedRuleViolation { tag, message, .. }
                    if tag == "loaded_imported" && message.contains("'value'")
            ))
    );
    drop(before);

    let implementation_path = project_root.join("app/implementation.py");
    db.add_file(
        implementation_path.as_str(),
        "def imported(value=None, **options): return value, options\n",
    )
    .expect("updated imported callable should be written");

    let updated_library =
        ScopedTemplateLibraries::from_project_inventory(template_library_catalog(&db, project))
            .resolved_libraries()
            .into_iter()
            .find(|library| library.module_name_str() == "app.templatetags.authored")
            .expect("updated authored Template Library should resolve");
    let after = library_tag_specs(&db, project, updated_library.id())
        .get("loaded_imported")
        .cloned()
        .expect("updated imported tag should keep its semantic spec");
    assert!(matches!(
        after.argument_syntax(),
        TagArgumentSyntax::Signature {
            parameters,
            variadic_keyword: Some(name),
            ..
        } if parameters.len() == 1
            && parameters[0].name == "value"
            && !parameters[0].requirement.is_required()
            && name == "options"
    ));
    assert!(
        collect_file_errors(&db, template_path.as_str())
            .expect("updated imported argument should validate")
            .is_empty()
    );
}

#[test]
fn semantic_grammar_vocabulary_indexes_definition_identities_and_openness() {
    let (db, project, _) = validation_project_database("grammar-vocabulary")
        .expect("project fixture should install into the test database");

    let vocabulary = semantic_grammar_vocabulary(&db, project);
    assert!(!vocabulary.is_open());
    let closer = vocabulary.closer_candidates("endif");
    let if_definition = closer
        .iter()
        .find(|definition| definition.name() == "if")
        .expect("builtin if should contribute its closer spelling");
    assert_eq!(
        if_definition.library().module(&db).as_str(),
        "django.template.defaulttags"
    );
    assert!(
        vocabulary
            .intermediate_candidates("else")
            .contains(if_definition)
    );
}

#[test]
fn feasible_backend_builtin_if_collision_stays_semantically_inconclusive() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nif FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['collision_tags']}}]\n";
    ProjectFixture::new("/proj")
        .django_settings_module("myproject.settings")
        .file("/proj/myproject/settings.py", settings)
        .file(
            "/proj/collision_tags.py",
            "from django import template\nregister = template.Library()\n@register.tag(name='if')\ndef custom_if(parser, token):\n    bits = token.split_contents()\n    if len(bits) != 1:\n        raise template.TemplateSyntaxError('no arguments')\n    body = parser.parse(('endcustom',))\n    return Node(body)\n",
        )
        .file(
            "/proj/templates/page.html",
            "{% if and %}body{% endif %}",
        )
        .install(&mut db)
        .expect("project fixture should install into the test database");

    let file = db
        .file(Utf8Path::new("/proj/templates/page.html"))
        .expect("fixture file should exist in the test database");
    assert!(
        tag_specs_for_file(&db, file).get("if").is_none(),
        "conflicting feasible definitions must not acquire builtin structure, arguments, or role"
    );
    let errors = collect_file_errors(&db, "/proj/templates/page.html")
        .expect("fixture file validation errors should be collected");
    assert!(
        !errors.iter().any(|error| matches!(
            error,
            ValidationError::UnclosedTag { .. }
                | ValidationError::OrphanedTag { .. }
                | ValidationError::OrphanedClosingTag { .. }
                | ValidationError::UnbalancedStructure { .. }
                | ValidationError::ExtractedRuleViolation { .. }
                | ValidationError::ExpressionSyntaxError { .. }
                | ValidationError::UnknownTag { .. }
        )),
        "inconclusive builtin-name meaning must not emit structural, argument, role-driven, or expression diagnostics: {errors:?}"
    );
}

#[test]
fn captured_closer_does_not_retain_a_colliding_standalone_spec() {
    let mut specs = builtin_tag_specs();
    specs.insert(
        "endif".to_string(),
        TagSpec::new(
            "test.tags".into(),
            None,
            Cow::Borrowed(&[]),
            djls_semantic::BodyAnalysis::Analyze,
        ),
    );
    let db = TestDatabase::new().with_projectless_tag_specs(specs);

    let captured_source = "{% if condition %}{% endif collision %}";
    db.add_file("/captured.html", captured_source)
        .expect("fixture file should be added to the test database");
    let captured_file = db
        .file(Utf8Path::new("/captured.html"))
        .expect("fixture file should exist in the test database");
    let captured_nodelist =
        parse_template(&db, captured_file).expect("captured closer fixture should parse");
    let captured_position = u32::try_from(
        captured_source
            .find("collision")
            .expect("captured closer fixture should contain its argument"),
    )
    .expect("captured closer argument offset should fit in u32");
    assert_eq!(
        tag_spec_at(
            &db,
            captured_file,
            captured_nodelist,
            captured_position,
            "endif",
        ),
        None,
        "a closer consumed by the open if contract must not retain the standalone endif spec"
    );

    let standalone_source = "{% endif collision %}";
    db.add_file("/standalone.html", standalone_source)
        .expect("fixture file should be added to the test database");
    let standalone_file = db
        .file(Utf8Path::new("/standalone.html"))
        .expect("fixture file should exist in the test database");
    let standalone_nodelist =
        parse_template(&db, standalone_file).expect("standalone fixture should parse");
    let standalone_position = u32::try_from(
        standalone_source
            .find("collision")
            .expect("standalone tag fixture should contain its argument"),
    )
    .expect("standalone tag argument offset should fit in u32");
    assert!(
        tag_spec_at(
            &db,
            standalone_file,
            standalone_nodelist,
            standalone_position,
            "endif",
        )
        .is_some(),
        "the standalone endif occurrence should retain its own definition"
    );
}

#[test]
fn captured_intermediate_does_not_apply_a_colliding_standalone_contract() {
    let mut specs = builtin_tag_specs();
    specs.insert(
        "else".to_string(),
        TagSpec::new(
            "test.loader".into(),
            None,
            Cow::Borrowed(&[]),
            djls_semantic::BodyAnalysis::Analyze,
        )
        .with_role(TagRole::TemplateLibraryLoader)
        .with_extracted_rules(
            TagRule {
                arg_constraints: vec![ArgumentCountConstraint::Exact(3)],
                ..TagRule::default()
            }
            .into(),
        ),
    );
    let db = TestDatabase::new().with_projectless_tag_specs(specs);

    let standalone_errors = collect_test_errors(&db, "{% else one %}")
        .expect("template validation errors should be collected");
    assert!(standalone_errors.iter().any(|error| matches!(
        error,
        ValidationError::ExtractedRuleViolation { tag, .. } if tag == "else"
    )));

    let captured_errors = collect_test_errors(&db, "{% if condition %}{% else one %}{% endif %}")
        .expect("template validation errors should be collected");
    assert!(
        !captured_errors.iter().any(|error| matches!(
            error,
            ValidationError::ExtractedRuleViolation { tag, .. }
                | ValidationError::UnknownLibrary { name: tag, .. }
                if tag == "else" || tag == "one"
        )),
        "captured else must use only the open if contract: {captured_errors:?}"
    );
}

fn extracted_block_db(module: &str) -> anyhow::Result<OsTestDatabase> {
    let (db, project, _) = validation_project_database("extracted-block")?;
    let module_name = format!("blog.templatetags.{module}");
    let library =
        ScopedTemplateLibraries::from_project_inventory(template_library_catalog(&db, project))
            .resolved_libraries()
            .into_iter()
            .find(|library| library.module_name_str() == module_name)
            .ok_or_else(|| anyhow::anyhow!("fixture library {module_name} was not discovered"))?;
    let library_specs = library_tag_specs(&db, project, library.id());
    let mut specs = TagSpecs::default();
    if let Some(spec) = library_specs.get("mystery") {
        specs.insert("mystery".to_string(), spec.clone());
    }
    Ok(db.with_projectless_tag_specs(specs))
}

fn extracted_unknown_block_db() -> anyhow::Result<OsTestDatabase> {
    extracted_block_db("dynamic_end")
}

fn extracted_self_named_block_db() -> anyhow::Result<OsTestDatabase> {
    extracted_block_db("self_named_end")
}

#[test]
fn extracted_unknown_block_does_not_require_synthesized_end_tag() {
    let mut db = extracted_unknown_block_db().expect("unknown block fixture should build");
    assert_eq!(
        db.projectless_tag_specs()
            .get("mystery")
            .and_then(|spec| spec.end_tag.as_ref())
            .map(|end_tag| end_tag.name.as_ref()),
        None::<&str>,
        "ambiguous extracted closer must stay unknown, not be synthesized"
    );

    let file = db
        .add_file("test.html", "{% load dynamic_end %}\n{% mystery %}\n")
        .expect("template source should be added");
    let errors = collect_validation_errors(&db, file);

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
    let mut db = extracted_self_named_block_db().expect("self-named block fixture should build");
    assert_eq!(
        db.projectless_tag_specs()
            .get("mystery")
            .and_then(|spec| spec.end_tag.as_ref())
            .map(|end_tag| end_tag.name.as_ref()),
        Some("endmystery")
    );

    let file = db
        .add_file("test.html", "{% load self_named_end %}\n{% mystery %}\n")
        .expect("template source should be added");
    let errors = collect_validation_errors(&db, file);

    assert!(
        errors.iter().any(|error| matches!(
            error,
            ValidationError::UnclosedTag { tag, .. } if tag == "mystery" && error.code() == "S100"
        )),
        "self-named extracted block tags should require their evidenced closer: {errors:?}"
    );
}

// Integration: Mixed diagnostics

// Snapshot tests for diagnostic output

// Extends validation (S122, S123)

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

fn format_failures(failures: &[FailureEntry]) -> Result<String, std::fmt::Error> {
    let mut out = String::new();
    for failure in failures.iter().take(20) {
        writeln!(out, "  {}:", failure.path)?;
        for error in &failure.errors {
            writeln!(out, "    - {error}")?;
        }
    }
    if failures.len() > 20 {
        writeln!(out, "  ... and {} more", failures.len() - 20)?;
    }
    Ok(out)
}

#[test]
fn corpus_templates_have_no_argument_false_positives() {
    let corpus = Corpus::require().expect("synced corpus should be available for corpus tests");

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

        let (specs, arities) = build_entry_specs(&corpus, &entry_dir)
            .expect("corpus entry tag and filter specs should build");
        let db = TestDatabase::new()
            .with_projectless_tag_specs(specs)
            .with_projectless_filter_arity_specs(arities);

        for (i, template_path) in templates.into_iter().enumerate() {
            let Ok(content) = fs::read_to_string(template_path.as_std_path()) else {
                continue;
            };

            let errors = collect_argument_validation_errors_with_revision(
                &db,
                "corpus_test.html",
                i as u64,
                &content,
            )
            .expect("corpus template argument errors should be collected");
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
        format_failures(&failures).expect("corpus failures should format")
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn loaded_unreadable_library_reports_each_load_argument_without_changing_validation() {
    let (db, _, project_root) = validation_project_database("unreadable-library")
        .expect("unreadable-library fixture should install");
    let open_path = project_root.join("open_tags.py");
    let template_root = project_root.join("templates");
    let open_template_path = template_root.join("open.html");
    let known_template_path = template_root.join("known.html");
    let mixed_template_path = template_root.join("mixed.html");
    let selective_template_path = template_root.join("selective.html");
    let repeated_template_path = template_root.join("repeated.html");
    let open_source = fs::read_to_string(open_path.as_std_path())
        .expect("open Template Library source should be readable");
    let open_template = fs::read_to_string(open_template_path.as_std_path())
        .expect("open template should be readable");
    let mixed_template = fs::read_to_string(mixed_template_path.as_std_path())
        .expect("mixed template should be readable");
    let selective_template = fs::read_to_string(selective_template_path.as_std_path())
        .expect("selective template should be readable");
    let repeated_template = fs::read_to_string(repeated_template_path.as_std_path())
        .expect("repeated template should be readable");

    let open_file = djls_source::path_to_file(&db, open_path.as_path())
        .expect("open Template Library source should exist");
    let unread_start = open_source
        .lines()
        .take(3)
        .map(|line| line.len() + 1)
        .sum::<usize>();
    let unread_length = open_source
        .lines()
        .nth(3)
        .expect("open source should contain the unread statement")
        .len();
    let expected_unread = vec![UnreadRegistration {
        span: djls_source::Span::saturating_from_bounds_usize(
            unread_start,
            unread_start + unread_length,
        ),
        shape: UnreadShape::RegistrationNameUnresolved,
    }];

    let open_errors =
        collect_file_errors(&db, open_template_path.as_str()).expect("open load should validate");
    let [
        ValidationError::UnreadableLibrary {
            library,
            span,
            registration_file,
            unread,
            ..
        },
    ] = open_errors.as_slice()
    else {
        panic!("expected one unreadable-library hint, got {open_errors:#?}");
    };
    assert_eq!(library, "open");
    assert_eq!(
        *span,
        djls_source::Span::saturating_from_bounds_usize(
            open_template
                .find("open")
                .expect("open load name should exist"),
            open_template
                .find("open")
                .expect("open load name should exist")
                + "open".len(),
        )
    );
    assert_eq!(*registration_file, open_file);
    assert_eq!(unread, &expected_unread);
    assert_eq!(
        open_errors[0].to_string(),
        "DJLS could not read a registration in `open_tags.py` at line 4 (the registered name cannot be resolved), so unrecognized tags and filters from `open` are not reported"
    );

    let known_errors =
        collect_file_errors(&db, known_template_path.as_str()).expect("known load should validate");
    assert!(known_errors.is_empty(), "{known_errors:#?}");

    let mixed_errors = collect_file_errors(&db, mixed_template_path.as_str())
        .expect("mixed full load should validate");
    let [ValidationError::UnreadableLibrary { span, .. }] = mixed_errors.as_slice() else {
        panic!("expected one hint for the open argument, got {mixed_errors:#?}");
    };
    let mixed_open = mixed_template
        .find("open")
        .expect("mixed load should contain open");
    assert_eq!(
        *span,
        djls_source::Span::saturating_from_bounds_usize(mixed_open, mixed_open + "open".len())
    );

    let selective_errors = collect_file_errors(&db, selective_template_path.as_str())
        .expect("selective load should validate");
    let [ValidationError::UnreadableLibrary { span, .. }] = selective_errors.as_slice() else {
        panic!("expected one hint for a selective load, got {selective_errors:#?}");
    };
    let selective_open = selective_template
        .find("open")
        .expect("selective load should contain open");
    assert_eq!(
        *span,
        djls_source::Span::saturating_from_bounds_usize(
            selective_open,
            selective_open + "open".len()
        )
    );

    let repeated_errors = collect_file_errors(&db, repeated_template_path.as_str())
        .expect("repeated loads should validate");
    let repeated_starts = repeated_errors
        .iter()
        .map(|error| {
            let ValidationError::UnreadableLibrary { span, .. } = error else {
                panic!("expected only unreadable-library hints, got {error:#?}");
            };
            span.start()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        repeated_starts,
        repeated_template
            .match_indices("open")
            .map(|(start, _)| u32::try_from(start).expect("template offset should fit in u32"))
            .collect::<Vec<_>>()
    );
}

#[test]
fn corpus_known_invalid_templates_produce_errors() {
    let corpus = Corpus::require().expect("synced corpus should be available for corpus tests");

    let Some(django_dir) = corpus.latest_package("django") else {
        eprintln!("No Django in corpus.");
        return;
    };

    let (specs, arities) = build_specs_from_extraction(&corpus, &django_dir)
        .expect("Django tag and filter specs should build from corpus extraction");

    let db = TestDatabase::new()
        .with_projectless_tag_specs(specs)
        .with_projectless_filter_arity_specs(arities);

    // for tag with wrong number of args
    let errors = collect_argument_validation_errors_with_revision(
        &db,
        "corpus_test.html",
        0,
        "{% for %}content{% endfor %}",
    )
    .expect("invalid for-tag errors should be collected");
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
    )
    .expect("invalid if-expression errors should be collected");
    let expr_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e, ValidationError::ExpressionSyntaxError { .. }))
        .collect();
    assert!(
        !expr_errors.is_empty(),
        "Expected expression syntax error for {{% if and x %}}"
    );
}

#[test]
fn corpus_stylesheet_requires_one_argument() {
    let corpus = Corpus::require().expect("synced corpus should be available");
    let (specs, _) =
        build_specs_from_extraction(&corpus, &corpus.root().join("repos/django-pipeline"))
            .expect("corpus specs should build");
    let db = TestDatabase::new().with_projectless_tag_specs(specs);
    assert_eq!(
        collect_errors(&db, "/control.html", "{% stylesheet 'main' %}")
            .expect("template validation should run"),
        []
    );
    let errors = collect_errors(&db, "/invalid.html", "{% stylesheet %}")
        .expect("template validation should run");
    assert!(
        matches!(
            errors.as_slice(),
            [ValidationError::ExtractedRuleViolation { tag, message, .. }]
                if tag == "stylesheet" && message.contains("requires exactly one argument")
        ),
        "{errors:?}"
    );
}

#[test]
fn corpus_javascript_requires_one_argument() {
    let corpus = Corpus::require().expect("synced corpus should be available");
    let (specs, _) =
        build_specs_from_extraction(&corpus, &corpus.root().join("repos/django-pipeline"))
            .expect("corpus specs should build");
    let db = TestDatabase::new().with_projectless_tag_specs(specs);
    let errors = collect_errors(&db, "/valid.html", "{% javascript 'main' %}")
        .expect("template validation should run");
    assert!(errors.is_empty(), "{errors:?}");
    for template in ["{% javascript %}", "{% javascript 'main' extra %}"] {
        let errors =
            collect_errors(&db, "/invalid.html", template).expect("template validation should run");
        assert!(
            matches!(
                errors.as_slice(),
                [ValidationError::ExtractedRuleViolation { tag, message, .. }]
                    if tag == "javascript" && message.contains("requires exactly one argument")
            ),
            "{errors:?}"
        );
    }
}

#[test]
fn corpus_compress_requires_a_known_output_mode() {
    let corpus = Corpus::require().expect("synced corpus should be available");
    let (specs, _) =
        build_specs_from_extraction(&corpus, &corpus.root().join("repos/django-compressor"))
            .expect("corpus specs should build");
    let db = TestDatabase::new().with_projectless_tag_specs(specs);
    assert_eq!(
        collect_errors(
            &db,
            "/control.html",
            "{% compress css file %}x{% endcompress %}"
        )
        .expect("template validation should run"),
        []
    );
    let errors = collect_errors(
        &db,
        "/invalid.html",
        "{% compress css bogus %}x{% endcompress %}",
    )
    .expect("template validation should run");
    assert!(
        matches!(
            errors.as_slice(),
            [ValidationError::ExtractedRuleViolation { tag, message, .. }]
                if tag == "compress" && message.contains("second argument must be 'file' or 'inline'")
        ),
        "{errors:?}"
    );
}

#[test]
fn corpus_show_placeholder_gets_context_from_django() {
    let corpus = Corpus::require().expect("synced corpus should be available");
    let (specs, _) = build_specs_from_extraction(&corpus, &corpus.root().join("repos/django-cms"))
        .expect("corpus specs should build");
    let db = TestDatabase::new().with_projectless_tag_specs(specs);
    assert_eq!(
        collect_errors(
            &db,
            "/control.html",
            "{% show_placeholder 'slot' page_id 'en' %}"
        )
        .expect("template validation should run"),
        []
    );
    // Django supplies context to _show_placeholder_by_id.
    let errors = collect_errors(&db, "/valid.html", "{% show_placeholder 'slot' page_id %}")
        .expect("template validation should run");
    assert!(errors.is_empty(), "{errors:?}");
    let errors = collect_errors(&db, "/invalid.html", "{% show_placeholder %}")
        .expect("template validation should run");
    assert!(matches!(
        errors.as_slice(),
        [ValidationError::ExtractedRuleViolation { .. }]
    ));
}

#[test]
fn corpus_eventsignal_rejects_extra_positional_arguments() {
    let corpus = Corpus::require().expect("synced corpus should be available");
    let (specs, _) = build_specs_from_extraction(&corpus, &corpus.root().join("repos/pretix"))
        .expect("corpus specs should build");
    let db = TestDatabase::new().with_projectless_tag_specs(specs);
    assert_eq!(
        collect_errors(
            &db,
            "/control.html",
            "{% eventsignal event 'signal.name' %}"
        )
        .expect("template validation should run"),
        []
    );
    // Django's parse_bits rejects extra positional arguments even with **kwargs.
    let errors = collect_errors(
        &db,
        "/invalid.html",
        "{% eventsignal event 'signal.name' extra %}",
    )
    .expect("template validation should run");
    assert!(
        matches!(
            errors.as_slice(),
            [ValidationError::ExtractedRuleViolation { tag, message, .. }]
                if tag == "eventsignal" && message.contains("received too many positional arguments")
        ),
        "{errors:?}"
    );
}

#[test]
fn corpus_activity_stream_curried_registration() {
    let corpus = Corpus::require().expect("synced corpus should be available");
    let root = corpus.root().join("repos/django-activity-stream");
    let extraction_db = djls_testing::OsTestDatabase::with_disk_roots([root.clone()]);
    let file = djls_source::path_to_file(
        &extraction_db,
        &root.join("actstream/templatetags/activity_tags.py"),
    )
    .expect("corpus source file should exist");
    file.try_source(&extraction_db)
        .expect("corpus source file should be readable");
    let module = djls_project::PythonModuleName::parse("actstream.templatetags.activity_tags")
        .expect("corpus module name should parse");
    let library = djls_project::TemplateLibraryId::new(&extraction_db, Some(file), module);
    assert!(
        djls_project::template_library_definition_facts(&extraction_db, library)
            .symbol(TemplateSymbolKind::Tag, "activity_stream")
            .is_some()
    );
    let (specs, _) =
        build_specs_from_extraction(&corpus, &root).expect("corpus specs should build");
    let db = TestDatabase::new().with_projectless_tag_specs(specs);
    let errors = collect_errors(&db, "/valid.html", "{% activity_stream 'actor' %}")
        .expect("template validation should run");
    assert!(errors.is_empty(), "{errors:?}");
    // Django's parse_bits rejects the missing stream_type argument.
    let errors = collect_errors(&db, "/invalid.html", "{% activity_stream %}")
        .expect("template validation should run");
    assert!(
        matches!(
            errors.as_slice(),
            [ValidationError::ExtractedRuleViolation { tag, .. }] if tag == "activity_stream"
        ),
        "{errors:?}"
    );
}

#[test]
fn corpus_element_requires_an_argument() {
    let corpus = Corpus::require().expect("synced corpus should be available");
    let (specs, _) =
        build_specs_from_extraction(&corpus, &corpus.root().join("repos/django-allauth"))
            .expect("corpus specs should build");
    let db = TestDatabase::new().with_projectless_tag_specs(specs);
    assert_eq!(
        collect_errors(
            &db,
            "/control.html",
            "{% element 'button' %}x{% endelement %}"
        )
        .expect("template validation should run"),
        []
    );
    // Missed diagnostic: helper-returned arguments and args[0] do not establish
    // an arity rule. allauth's empty call raises IndexError at args[0], not
    // TemplateSyntaxError; its explicit multi-argument error starts "Usage:".
    assert_eq!(
        collect_errors(&db, "/invalid.html", "{% element %}x{% endelement %}")
            .expect("template validation should run"),
        []
    );
}

#[test]
fn corpus_with_data_requires_as_keyword() {
    let corpus = Corpus::require().expect("synced corpus should be available");
    let (specs, _) =
        build_specs_from_extraction(&corpus, &corpus.root().join("repos/django-sekizai"))
            .expect("corpus specs should build");
    let db = TestDatabase::new().with_projectless_tag_specs(specs);
    // False positives: the class parser has no tag or closing-tag spec.
    // WithData.options requires the literal 'as'; classytags supplies the
    // invalid-call message, whose source is not part of the pinned corpus.
    for template in [
        "{% with_data 'css' as values %}x{% end_with_data %}",
        "{% with_data 'css' into values %}x{% end_with_data %}",
    ] {
        let errors =
            collect_errors(&db, "/case.html", template).expect("template validation should run");
        let [
            ValidationError::UnknownTag { tag: opening, .. },
            ValidationError::UnknownTag { tag: closing, .. },
        ] = errors.as_slice()
        else {
            panic!("expected the class-parser spec gap, got {errors:?}");
        };
        assert_eq!(opening, "with_data");
        assert_eq!(closing, "end_with_data");
    }
}

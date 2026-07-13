use djls_project::EffectiveDefinitionLibrary;
use djls_project::EnvironmentSymbolLookup;
use djls_project::LibraryName;
use djls_project::LoadableLibraryLookup;
use djls_project::MissingLibraryLookup;
use djls_project::PythonModuleName;
use djls_project::SymbolDefinition;
use djls_project::TemplateEnvironment;
use djls_project::TemplateLibraries;
use djls_project::TemplateSymbol;
use djls_project::TemplateSymbolAvailability;
use djls_project::TemplateSymbolKind;
use djls_project::TemplateSymbolLookup;
use djls_project::TemplateSymbolName;
use djls_project::template_libraries;
use djls_project::testing;
use djls_project::testing::TemplateBackendLibrariesInput;
use djls_project::testing::TemplateLibraryInput;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;

fn module(name: &str) -> PythonModuleName {
    PythonModuleName::parse(name).unwrap()
}

fn library_name(name: &str) -> LibraryName {
    LibraryName::parse(name).unwrap()
}

fn symbol(kind: TemplateSymbolKind, name: &str, doc: Option<&str>) -> TemplateSymbol {
    TemplateSymbol {
        kind,
        name: TemplateSymbolName::parse(name).unwrap(),
        definition: SymbolDefinition::Unknown,
        doc: doc.map(str::to_string),
    }
}

fn builtin(module: &str, symbols: Vec<TemplateSymbol>) -> TemplateLibraryInput {
    TemplateLibraryInput::Builtin {
        module: self::module(module),
        symbols,
    }
}

fn installed(load_name: &str, module: &str, symbols: Vec<TemplateSymbol>) -> TemplateLibraryInput {
    TemplateLibraryInput::Installed {
        load_name: library_name(load_name),
        module: self::module(module),
        symbols,
    }
}

fn available(
    load_name: &str,
    app: &str,
    module: &str,
    symbols: Vec<TemplateSymbol>,
) -> TemplateLibraryInput {
    TemplateLibraryInput::Available {
        load_name: library_name(load_name),
        app: self::module(app),
        module: self::module(module),
        symbols,
    }
}

fn libraries(open: bool, inputs: Vec<TemplateLibraryInput>) -> TemplateLibraries {
    let db = TestDatabase::new();
    if open {
        testing::template_libraries_with_omissions(&db, inputs)
    } else {
        testing::template_libraries(&db, inputs)
    }
}

fn backend(loadable: Vec<(&str, &str)>, builtins: Vec<&str>) -> TemplateBackendLibrariesInput {
    TemplateBackendLibrariesInput {
        loadable: loadable
            .into_iter()
            .map(|(name, module)| (library_name(name), self::module(module)))
            .collect(),
        builtins: builtins.into_iter().map(self::module).collect(),
    }
}

fn configured_libraries(
    open: bool,
    inputs: Vec<TemplateLibraryInput>,
    configurations: Vec<Vec<TemplateBackendLibrariesInput>>,
) -> TemplateLibraries {
    let db = TestDatabase::new();
    if open {
        testing::template_libraries_with_configuration_omissions(&db, inputs, configurations)
    } else {
        testing::template_libraries_with_configurations(&db, inputs, configurations)
    }
}

#[test]
fn closed_and_open_misses_are_distinct() {
    let name = library_name("missing");
    assert_eq!(
        libraries(false, Vec::new()).loadable_library(&name),
        LoadableLibraryLookup::Absent
    );
    assert_eq!(
        libraries(true, Vec::new()).loadable_library(&name),
        LoadableLibraryLookup::Inconclusive(Vec::new())
    );
}

#[test]
fn configuration_lookup_distinguishes_unanimous_disagreement_and_open_remainder() {
    let inputs = vec![
        installed("shared", "project.alpha", Vec::new()),
        installed("shared", "project.beta", Vec::new()),
    ];
    let unanimous = configured_libraries(
        false,
        inputs.clone(),
        vec![vec![backend(vec![("shared", "project.alpha")], vec![])]],
    );
    assert!(matches!(
        unanimous.loadable_library(&library_name("shared")),
        LoadableLibraryLookup::Found(library) if library.module_name_str() == "project.alpha"
    ));

    let disagreement = configured_libraries(
        false,
        inputs.clone(),
        vec![vec![
            backend(vec![("shared", "project.alpha")], vec![]),
            backend(vec![("shared", "project.beta")], vec![]),
        ]],
    );
    assert!(matches!(
        disagreement.loadable_library(&library_name("shared")),
        LoadableLibraryLookup::Ambiguous(records) if records.len() == 2
    ));

    let present_absent = configured_libraries(
        false,
        inputs.clone(),
        vec![vec![
            backend(vec![("shared", "project.alpha")], vec![]),
            backend(vec![], vec![]),
        ]],
    );
    assert!(matches!(
        present_absent.loadable_library(&library_name("shared")),
        LoadableLibraryLookup::Ambiguous(records) if records.len() == 1
    ));

    let open = configured_libraries(
        true,
        inputs,
        vec![vec![backend(vec![("shared", "project.alpha")], vec![])]],
    );
    assert!(matches!(
        open.loadable_library(&library_name("shared")),
        LoadableLibraryLookup::Inconclusive(records) if records.len() == 1
    ));
}

#[test]
fn symbol_join_distinguishes_unanimous_and_partial_ambiguous_libraries() {
    let inputs = vec![
        installed(
            "shared",
            "project.alpha",
            vec![
                symbol(TemplateSymbolKind::Tag, "all_tag", None),
                symbol(TemplateSymbolKind::Tag, "one_tag", None),
                symbol(TemplateSymbolKind::Filter, "all_filter", None),
                symbol(TemplateSymbolKind::Filter, "one_filter", None),
            ],
        ),
        installed(
            "shared",
            "project.beta",
            vec![
                symbol(TemplateSymbolKind::Tag, "all_tag", None),
                symbol(TemplateSymbolKind::Filter, "all_filter", None),
            ],
        ),
    ];
    let libraries = configured_libraries(
        false,
        inputs,
        vec![vec![
            backend(vec![("shared", "project.alpha")], vec![]),
            backend(vec![("shared", "project.beta")], vec![]),
        ]],
    );

    assert_eq!(
        libraries.environment_symbol_lookup("all_tag", TemplateSymbolKind::Tag),
        EnvironmentSymbolLookup::RequiresLoad(vec![library_name("shared")])
    );
    assert_eq!(
        libraries.environment_symbol_lookup("one_tag", TemplateSymbolKind::Tag),
        EnvironmentSymbolLookup::Inconclusive
    );
    assert_eq!(
        libraries.environment_symbol_lookup("all_filter", TemplateSymbolKind::Filter),
        EnvironmentSymbolLookup::RequiresLoad(vec![library_name("shared")])
    );
    assert_eq!(
        libraries.environment_symbol_lookup("one_filter", TemplateSymbolKind::Filter),
        EnvironmentSymbolLookup::Inconclusive
    );
}

#[test]
fn effective_definition_preserves_absence_and_load_precedence_per_backend() {
    let db = TestDatabase::new();
    let inventory = configured_libraries(
        false,
        vec![
            builtin(
                "django.template.defaulttags",
                vec![symbol(TemplateSymbolKind::Tag, "if", None)],
            ),
            installed(
                "alpha",
                "project.alpha",
                vec![symbol(TemplateSymbolKind::Tag, "if", None)],
            ),
            installed(
                "beta",
                "project.beta",
                vec![symbol(TemplateSymbolKind::Tag, "if", None)],
            ),
        ],
        vec![vec![
            backend(
                vec![("alpha", "project.alpha"), ("beta", "project.beta")],
                vec!["django.template.defaulttags"],
            ),
            backend(
                vec![("alpha", "project.alpha"), ("beta", "project.beta")],
                vec![],
            ),
        ]],
    );
    let environment = TemplateEnvironment::from_project_inventory(&inventory);

    let unloaded =
        environment.effective_definition_libraries(&db, "if", TemplateSymbolKind::Tag, &[]);
    assert!(matches!(unloaded.as_slice(), [
        EffectiveDefinitionLibrary::Known(Some(library)),
        EffectiveDefinitionLibrary::Known(None),
    ] if library.module_name_str() == "django.template.defaulttags"));

    let loaded =
        environment.effective_definition_libraries(&db, "if", TemplateSymbolKind::Tag, &["alpha"]);
    assert!(loaded.iter().all(|alternative| matches!(
        alternative,
        EffectiveDefinitionLibrary::Known(Some(library))
            if library.module_name_str() == "project.alpha"
    )));
}

#[test]
fn source_less_configured_library_keeps_keyed_structural_facts_without_origin() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .django_settings_module("project.settings")
        .tag_specs(djls_conf::TagSpecDef {
            libraries: vec![djls_conf::TagLibraryDef {
                module: "missing.panel_tags".to_string(),
                requires_engine: None,
                tags: vec![djls_conf::TagDef {
                    name: "panel".to_string(),
                    tag_type: djls_conf::TagTypeDef::Block,
                    end: None,
                    intermediates: Vec::new(),
                    args: Vec::new(),
                    extra: None,
                }],
                extra: None,
            }],
            ..djls_conf::TagSpecDef::default()
        })
        .file(
            "/project/project/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {'panels': 'missing.panel_tags'}}}]\n",
        )
        .build(&db);

    let libraries = template_libraries(&db, project);
    let LoadableLibraryLookup::Found(library) = libraries.loadable_library_str("panels") else {
        panic!("configured library should remain definitively loadable");
    };
    assert_eq!(library.module_name_str(), "missing.panel_tags");
    assert!(library.source_file().is_none());
    assert!(
        library
            .symbol(TemplateSymbolKind::Tag, "panel")
            .is_some_and(|symbol| matches!(symbol.definition, SymbolDefinition::Unknown))
    );
    assert_eq!(library.key(&db).file(&db), None);
}

#[test]
fn source_less_alias_keeps_missing_same_named_available_app_symbols_inconclusive() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .django_settings_module("project.settings")
        .file(
            "/project/project/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {'shared': 'missing.shared'}}}]\n",
        )
        .file("/project/available_app/__init__.py", "")
        .file("/project/available_app/templatetags/__init__.py", "")
        .file(
            "/project/available_app/templatetags/shared.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef shared_tag(): pass\n@register.filter\ndef shared_filter(value): return value\n",
        )
        .build(&db);

    let libraries = template_libraries(&db, project);
    let LoadableLibraryLookup::Found(library) = libraries.loadable_library_str("shared") else {
        panic!("the configured source-less alias should be definitively loadable");
    };
    assert!(library.source_file().is_none());
    assert!(library.symbol_inventory_is_open());
    assert_eq!(
        libraries.environment_symbol_lookup("shared_tag", TemplateSymbolKind::Tag),
        EnvironmentSymbolLookup::Inconclusive
    );
    assert_eq!(
        libraries.environment_symbol_lookup("shared_filter", TemplateSymbolKind::Filter),
        EnvironmentSymbolLookup::Inconclusive
    );
    assert_eq!(
        libraries.template_symbol_lookup("shared_tag", TemplateSymbolKind::Tag),
        TemplateSymbolLookup::Inconclusive
    );
    assert_eq!(
        libraries.template_symbol_lookup("shared_filter", TemplateSymbolKind::Filter),
        TemplateSymbolLookup::Inconclusive
    );

    let environment = TemplateEnvironment::from_project_inventory(libraries);
    for kind in [TemplateSymbolKind::Tag, TemplateSymbolKind::Filter] {
        let name = match kind {
            TemplateSymbolKind::Tag => "shared_tag",
            TemplateSymbolKind::Filter => "shared_filter",
        };
        assert!(matches!(
            environment
                .effective_definition_libraries(&db, name, kind, &["shared"])
                .as_slice(),
            [EffectiveDefinitionLibrary::Unobserved(candidate)]
                if candidate.key(&db) == library.key(&db)
        ));
    }
}

#[test]
fn exact_alias_after_unknown_key_is_definitive_while_other_names_stay_open() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .django_settings_module("project.settings")
        .file(
            "/project/project/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {unknown_name: 'unknown.tags', 'shared': 'project_tags'}}}]\n",
        )
        .file(
            "/project/project_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef certain(): pass\n",
        )
        .build(&db);

    let libraries = template_libraries(&db, project);
    assert!(matches!(
        libraries.loadable_library_str("shared"),
        LoadableLibraryLookup::Found(library)
            if library.module_name_str() == "project_tags"
    ));
    assert_eq!(
        libraries.loadable_library_str("other"),
        LoadableLibraryLookup::Inconclusive(Vec::new())
    );
}

#[test]
fn definite_load_restores_certainty_after_uncertain_builtins() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .django_settings_module("project.settings")
        .file(
            "/project/project/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {'certain': 'project_tags'}, 'builtins': [*UNKNOWN]}}]\n",
        )
        .file(
            "/project/project_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef restored(): pass\n",
        )
        .build(&db);
    let inventory = template_libraries(&db, project);
    let environment = TemplateEnvironment::from_project_inventory(inventory);

    let definitions = environment.effective_definition_libraries(
        &db,
        "restored",
        TemplateSymbolKind::Tag,
        &["certain"],
    );
    assert!(definitions.iter().all(|definition| matches!(
        definition,
        EffectiveDefinitionLibrary::Known(Some(library))
            if library.module_name_str() == "project_tags"
    )));
}

#[test]
fn installed_duplicate_load_name_uses_last_record() {
    let libraries = libraries(
        false,
        vec![
            installed("custom", "project.templatetags.original", Vec::new()),
            installed("custom", "project.templatetags.replacement", Vec::new()),
        ],
    );
    let LoadableLibraryLookup::Found(library) = libraries.loadable_library(&library_name("custom"))
    else {
        panic!("expected a definitive installed library");
    };
    assert_eq!(
        library.module_name_str(),
        "project.templatetags.replacement"
    );
}

#[test]
fn available_symbol_guidance_survives_open_remainder() {
    let app = module("available_app");
    let load_name = library_name("extra_tags");
    let libraries = libraries(
        true,
        vec![available(
            "extra_tags",
            "available_app",
            "available_app.templatetags.extra_tags",
            vec![symbol(TemplateSymbolKind::Tag, "extra", None)],
        )],
    );
    assert_eq!(
        libraries.template_symbol_lookup("extra", TemplateSymbolKind::Tag),
        TemplateSymbolLookup::FoundInApp { app, load_name }
    );
}

#[test]
fn available_library_guidance_is_sorted_and_deduplicated() {
    let libraries = libraries(
        false,
        vec![
            available("shared", "beta", "beta.templatetags.shared", Vec::new()),
            available(
                "shared",
                "alpha",
                "alpha.templatetags.shared_extra",
                Vec::new(),
            ),
            available("shared", "alpha", "alpha.templatetags.shared", Vec::new()),
        ],
    );
    let MissingLibraryLookup::FoundInApps(apps) =
        libraries.missing_library_lookup(&library_name("shared"))
    else {
        panic!("shared should have available app candidates");
    };
    assert_eq!(apps.primary(), &module("alpha"));
    assert_eq!(apps.as_slice(), [module("alpha"), module("beta")]);
}

#[test]
fn known_symbol_candidates_preserve_builtin_and_load_semantics() {
    let libraries = libraries(
        false,
        vec![
            builtin(
                "z_first",
                vec![symbol(
                    TemplateSymbolKind::Filter,
                    "duplicate",
                    Some("first"),
                )],
            ),
            builtin(
                "a_second",
                vec![symbol(
                    TemplateSymbolKind::Filter,
                    "duplicate",
                    Some("second"),
                )],
            ),
            installed(
                "humanize",
                "django.contrib.humanize.templatetags.humanize",
                vec![symbol(TemplateSymbolKind::Filter, "intcomma", None)],
            ),
        ],
    );
    let candidates = libraries.template_symbol_candidates(TemplateSymbolKind::Filter);
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].symbol.name(), "duplicate");
    let duplicate_library = libraries
        .resolved_libraries()
        .find(|library| library.module_name_str() == "a_second")
        .expect("indexed builtin should be present");
    assert!(
        duplicate_library
            .symbol(TemplateSymbolKind::Filter, "duplicate")
            .is_some()
    );
    assert!(
        duplicate_library
            .symbol(TemplateSymbolKind::Tag, "duplicate")
            .is_none()
    );
    assert_eq!(candidates[0].symbol.doc.as_deref(), Some("second"));
    assert!(matches!(
        candidates[0].availability,
        TemplateSymbolAvailability::Builtin { .. }
    ));
    assert!(matches!(
        &candidates[1].availability,
        TemplateSymbolAvailability::RequiresLoad { load_name }
            if load_name.as_str() == "humanize"
    ));
}

#[test]
fn resolved_library_inventory_deduplicates_identical_builtin_identity() {
    let db = TestDatabase::new();
    let libraries = testing::template_libraries(
        &db,
        vec![
            builtin("django.template.defaulttags", Vec::new()),
            builtin("project.builtins", Vec::new()),
            builtin("django.template.defaulttags", Vec::new()),
        ],
    );
    let modules: Vec<_> = libraries
        .resolved_libraries()
        .map(djls_project::TemplateLibrary::module_name_str)
        .collect();
    assert_eq!(
        modules,
        vec!["django.template.defaulttags", "project.builtins"]
    );
    assert!(
        libraries
            .resolved_libraries()
            .all(|library| library.source_file().is_none()),
        "configured test evidence must not invent source origins"
    );
}

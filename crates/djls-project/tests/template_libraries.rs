use std::io;

use djls_conf::TagDef;
use djls_conf::TagLibraryDef;
use djls_conf::TagSpecDef;
use djls_conf::TagTypeDef;
use djls_project::AppTemplateSymbolLookup;
use djls_project::EffectiveDefinitionLibrary;
use djls_project::LibraryName;
use djls_project::LoadableLibraryLookup;
use djls_project::MissingTemplateLibraryLookup;
use djls_project::PythonModuleName;
use djls_project::ScopedTemplateLibraries;
use djls_project::ScopedTemplateSymbolLookup;
use djls_project::SymbolDefinition;
use djls_project::TemplateLibraryAppCandidates;
use djls_project::TemplateLibraryCatalog;
use djls_project::TemplateSymbol;
use djls_project::TemplateSymbolAvailability;
use djls_project::TemplateSymbolKind;
use djls_project::TemplateSymbolName;
use djls_project::template_library_catalog;
use djls_project::testing;
use djls_project::testing::TemplateBackendLibrariesInput;
use djls_project::testing::TemplateLibraryInput;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

fn module(name: &str) -> TestResult<PythonModuleName> {
    Ok(PythonModuleName::parse(name)?)
}

fn library_name(name: &str) -> TestResult<LibraryName> {
    Ok(LibraryName::parse(name)?)
}

fn symbol(kind: TemplateSymbolKind, name: &str, doc: Option<&str>) -> TestResult<TemplateSymbol> {
    Ok(TemplateSymbol {
        kind,
        name: TemplateSymbolName::parse(name)?,
        definition: SymbolDefinition::Unknown,
        doc: doc.map(str::to_string),
    })
}

fn builtin(
    module_name: &str,
    symbols: Vec<TestResult<TemplateSymbol>>,
) -> TestResult<TemplateLibraryInput> {
    Ok(TemplateLibraryInput::Builtin {
        module: module(module_name)?,
        symbols: symbols.into_iter().collect::<TestResult<_>>()?,
    })
}

fn loadable(
    load_name: &str,
    module_name: &str,
    symbols: Vec<TestResult<TemplateSymbol>>,
) -> TestResult<TemplateLibraryInput> {
    Ok(TemplateLibraryInput::Loadable {
        load_name: library_name(load_name)?,
        module: module(module_name)?,
        symbols: symbols.into_iter().collect::<TestResult<_>>()?,
    })
}

fn available_in_app(
    load_name: &str,
    app: &str,
    module_name: &str,
    symbols: Vec<TestResult<TemplateSymbol>>,
) -> TestResult<TemplateLibraryInput> {
    Ok(TemplateLibraryInput::AvailableInApp {
        load_name: library_name(load_name)?,
        app: module(app)?,
        module: module(module_name)?,
        symbols: symbols.into_iter().collect::<TestResult<_>>()?,
    })
}

fn libraries(open: bool, inputs: Vec<TemplateLibraryInput>) -> TemplateLibraryCatalog {
    let db = TestDatabase::new();
    if open {
        testing::template_library_catalog_with_omissions(&db, inputs)
    } else {
        testing::template_library_catalog(&db, inputs)
    }
}

fn backend(
    loadable: Vec<(&str, &str)>,
    builtins: Vec<&str>,
) -> TestResult<TemplateBackendLibrariesInput> {
    let loadable = loadable
        .into_iter()
        .map(|(name, module_name)| Ok((library_name(name)?, module(module_name)?)))
        .collect::<TestResult<_>>()?;
    let builtins = builtins
        .into_iter()
        .map(module)
        .collect::<TestResult<_>>()?;
    Ok(TemplateBackendLibrariesInput { loadable, builtins })
}

fn project_inventory(libraries: &TemplateLibraryCatalog) -> ScopedTemplateLibraries<'_> {
    ScopedTemplateLibraries::from_project_inventory(libraries)
}

fn available_in_app_candidates(
    lookup: MissingTemplateLibraryLookup,
) -> Result<TemplateLibraryAppCandidates, io::Error> {
    match lookup {
        MissingTemplateLibraryLookup::FoundInApps(candidates) => Ok(candidates),
        MissingTemplateLibraryLookup::Absent => {
            Err(io::Error::other("template library has no app candidates"))
        }
        MissingTemplateLibraryLookup::Inconclusive => Err(io::Error::other(
            "template library app candidates are inconclusive",
        )),
    }
}

fn configured_libraries(
    open: bool,
    inputs: Vec<TemplateLibraryInput>,
    settings_cases: Vec<Vec<TemplateBackendLibrariesInput>>,
) -> TestResult<TemplateLibraryCatalog> {
    let db = TestDatabase::new();
    Ok(if open {
        testing::template_library_catalog_with_settings_case_omissions(&db, inputs, settings_cases)?
    } else {
        testing::template_library_catalog_with_settings_cases(&db, inputs, settings_cases)?
    })
}

#[test]
fn closed_and_open_misses_are_distinct() {
    let name = library_name("missing").expect("test library name should be valid");
    let closed = libraries(false, Vec::new());
    assert_eq!(
        project_inventory(&closed).loadable_library(&name),
        LoadableLibraryLookup::Absent
    );
    let open = libraries(true, Vec::new());
    assert_eq!(
        project_inventory(&open).loadable_library(&name),
        LoadableLibraryLookup::Inconclusive(Vec::new())
    );
}

#[test]
fn settings_case_lookup_distinguishes_unanimous_disagreement_and_open_remainder() {
    let inputs = vec![
        loadable("shared", "project.alpha", Vec::new())
            .expect("loadable library fixture should be valid"),
        loadable("shared", "project.beta", Vec::new())
            .expect("loadable library fixture should be valid"),
    ];
    let unanimous = configured_libraries(
        false,
        inputs.clone(),
        vec![vec![
            backend(vec![("shared", "project.alpha")], vec![])
                .expect("template backend fixture should be valid"),
        ]],
    )
    .expect("template library settings fixture should be valid");
    assert!(matches!(
        project_inventory(&unanimous).loadable_library(&library_name("shared").expect("test library name should be valid")),
        LoadableLibraryLookup::Found(library) if library.module_name_str() == "project.alpha"
    ));

    let disagreement = configured_libraries(
        false,
        inputs.clone(),
        vec![vec![
            backend(vec![("shared", "project.alpha")], vec![])
                .expect("template backend fixture should be valid"),
            backend(vec![("shared", "project.beta")], vec![])
                .expect("template backend fixture should be valid"),
        ]],
    )
    .expect("template library settings fixture should be valid");
    assert!(matches!(
        project_inventory(&disagreement).loadable_library(&library_name("shared").expect("test library name should be valid")),
        LoadableLibraryLookup::Ambiguous(records) if records.len() == 2
    ));

    let present_absent = configured_libraries(
        false,
        inputs.clone(),
        vec![vec![
            backend(vec![("shared", "project.alpha")], vec![])
                .expect("template backend fixture should be valid"),
            backend(vec![], vec![]).expect("template backend fixture should be valid"),
        ]],
    )
    .expect("template library settings fixture should be valid");
    assert!(matches!(
        project_inventory(&present_absent).loadable_library(&library_name("shared").expect("test library name should be valid")),
        LoadableLibraryLookup::Ambiguous(records) if records.len() == 1
    ));

    let open = configured_libraries(
        true,
        inputs,
        vec![vec![
            backend(vec![("shared", "project.alpha")], vec![])
                .expect("template backend fixture should be valid"),
        ]],
    )
    .expect("template library settings fixture should be valid");
    assert!(matches!(
        project_inventory(&open).loadable_library(&library_name("shared").expect("test library name should be valid")),
        LoadableLibraryLookup::Inconclusive(records) if records.len() == 1
    ));
}

#[test]
fn symbol_join_distinguishes_unanimous_and_partial_ambiguous_libraries() {
    let inputs = vec![
        loadable(
            "shared",
            "project.alpha",
            vec![
                symbol(TemplateSymbolKind::Tag, "all_tag", None),
                symbol(TemplateSymbolKind::Tag, "one_tag", None),
                symbol(TemplateSymbolKind::Filter, "all_filter", None),
                symbol(TemplateSymbolKind::Filter, "one_filter", None),
            ],
        )
        .expect("loadable library fixture should be valid"),
        loadable(
            "shared",
            "project.beta",
            vec![
                symbol(TemplateSymbolKind::Tag, "all_tag", None),
                symbol(TemplateSymbolKind::Filter, "all_filter", None),
            ],
        )
        .expect("loadable library fixture should be valid"),
    ];
    let libraries = configured_libraries(
        false,
        inputs,
        vec![vec![
            backend(vec![("shared", "project.alpha")], vec![])
                .expect("template backend fixture should be valid"),
            backend(vec![("shared", "project.beta")], vec![])
                .expect("template backend fixture should be valid"),
        ]],
    )
    .expect("template library settings fixture should be valid");

    assert_eq!(
        project_inventory(&libraries).symbol("all_tag", TemplateSymbolKind::Tag),
        ScopedTemplateSymbolLookup::RequiresLoad(vec![
            library_name("shared").expect("test library name should be valid")
        ])
    );
    assert_eq!(
        project_inventory(&libraries).symbol("one_tag", TemplateSymbolKind::Tag),
        ScopedTemplateSymbolLookup::Inconclusive
    );
    assert_eq!(
        project_inventory(&libraries).symbol("all_filter", TemplateSymbolKind::Filter),
        ScopedTemplateSymbolLookup::RequiresLoad(vec![
            library_name("shared").expect("test library name should be valid")
        ])
    );
    assert_eq!(
        project_inventory(&libraries).symbol("one_filter", TemplateSymbolKind::Filter),
        ScopedTemplateSymbolLookup::Inconclusive
    );
}

#[test]
fn effective_definition_preserves_absence_and_load_precedence_per_backend() {
    let inventory = configured_libraries(
        false,
        vec![
            builtin(
                "django.template.defaulttags",
                vec![symbol(TemplateSymbolKind::Tag, "if", None)],
            )
            .expect("builtin library fixture should be valid"),
            loadable(
                "alpha",
                "project.alpha",
                vec![symbol(TemplateSymbolKind::Tag, "if", None)],
            )
            .expect("loadable library fixture should be valid"),
            loadable(
                "beta",
                "project.beta",
                vec![symbol(TemplateSymbolKind::Tag, "if", None)],
            )
            .expect("loadable library fixture should be valid"),
        ],
        vec![vec![
            backend(
                vec![("alpha", "project.alpha"), ("beta", "project.beta")],
                vec!["django.template.defaulttags"],
            )
            .expect("template backend fixture should be valid"),
            backend(
                vec![("alpha", "project.alpha"), ("beta", "project.beta")],
                vec![],
            )
            .expect("template backend fixture should be valid"),
        ]],
    )
    .expect("template library settings fixture should be valid");
    let scoped_libraries = ScopedTemplateLibraries::from_project_inventory(&inventory);

    let unloaded =
        scoped_libraries.effective_definition_libraries("if", TemplateSymbolKind::Tag, &[]);
    assert!(matches!(unloaded.as_slice(), [
        EffectiveDefinitionLibrary::Known(Some(library)),
        EffectiveDefinitionLibrary::Known(None),
    ] if library.module_name_str() == "django.template.defaulttags"));
    let mut streamed_unloaded = Vec::new();
    scoped_libraries.for_each_effective_definition_library(
        "if",
        TemplateSymbolKind::Tag,
        &[],
        |definition| streamed_unloaded.push(definition),
    );
    assert_eq!(streamed_unloaded, unloaded);

    let loaded =
        scoped_libraries.effective_definition_libraries("if", TemplateSymbolKind::Tag, &["alpha"]);
    assert!(loaded.iter().all(|alternative| matches!(
        alternative,
        EffectiveDefinitionLibrary::Known(Some(library))
            if library.module_name_str() == "project.alpha"
    )));
    let materialized_chains = scoped_libraries.library_chains(&["alpha"]);
    let mut folded_chains = Vec::new();
    scoped_libraries.fold_library_chains(&["alpha"], Vec::new, Vec::push, |chain| {
        folded_chains.push(chain);
    });
    assert_eq!(
        folded_chains,
        materialized_chains
            .iter()
            .map(|chain| chain.steps().to_vec())
            .collect::<Vec<_>>()
    );
}

#[test]
fn source_less_configured_library_keeps_keyed_structural_facts_without_origin() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .django_settings_module("project.settings")
        .tag_specs(TagSpecDef {
            libraries: vec![TagLibraryDef {
                module: "missing.panel_tags".to_string(),
                requires_engine: None,
                tags: vec![TagDef {
                    name: "panel".to_string(),
                    tag_type: TagTypeDef::Block,
                    end: None,
                    intermediates: Vec::new(),
                    args: Vec::new(),
                    extra: None,
                }],
                extra: None,
            }],
            ..TagSpecDef::default()
        })
        .file(
            "/project/project/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {'panels': 'missing.panel_tags'}}}]\n",
        )
        .build(&db)
        .expect("missing-library project fixture should build");

    let libraries = template_library_catalog(&db, project);
    let library = project_inventory(libraries)
        .loadable_library_str("panels")
        .found()
        .expect("configured panels library should remain definitively loadable");
    assert_eq!(library.module_name_str(), "missing.panel_tags");
    assert!(library.source_file().is_none());
    assert!(
        library
            .symbol(TemplateSymbolKind::Tag, "panel")
            .is_some_and(|symbol| matches!(symbol.definition, SymbolDefinition::Unknown))
    );
    assert_eq!(library.id().file(&db), None);
}

#[test]
fn source_less_alias_keeps_missing_same_named_available_in_app_symbols_inconclusive() {
    let db = TestDatabase::new();
    let project = ProjectFixture::new("/project")
        .django_settings_module("project.settings")
        .file(
            "/project/project/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'OPTIONS': {'libraries': {'shared': 'missing.shared'}}}]\n",
        )
        .file("/project/available_in_app/__init__.py", "")
        .file("/project/available_in_app/templatetags/__init__.py", "")
        .file(
            "/project/available_in_app/templatetags/shared.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef shared_tag(): pass\n@register.filter\ndef shared_filter(value): return value\n",
        )
        .build(&db)
        .expect("shared-library project fixture should build");

    let libraries = template_library_catalog(&db, project);
    let scoped_libraries = project_inventory(libraries);
    let library = scoped_libraries
        .loadable_library_str("shared")
        .found()
        .expect("configured source-less shared alias should be definitively loadable");
    assert!(library.source_file().is_none());
    assert!(library.symbols_are_unobserved());
    assert_eq!(
        scoped_libraries.symbol("shared_tag", TemplateSymbolKind::Tag),
        ScopedTemplateSymbolLookup::Inconclusive
    );
    assert_eq!(
        scoped_libraries.symbol("shared_filter", TemplateSymbolKind::Filter),
        ScopedTemplateSymbolLookup::Inconclusive
    );
    assert_eq!(
        scoped_libraries.available_in_app_symbol("shared_tag", TemplateSymbolKind::Tag),
        AppTemplateSymbolLookup::Inconclusive
    );
    assert_eq!(
        scoped_libraries.available_in_app_symbol("shared_filter", TemplateSymbolKind::Filter),
        AppTemplateSymbolLookup::Inconclusive
    );

    for kind in [TemplateSymbolKind::Tag, TemplateSymbolKind::Filter] {
        let name = match kind {
            TemplateSymbolKind::Tag => "shared_tag",
            TemplateSymbolKind::Filter => "shared_filter",
        };
        assert!(matches!(
            scoped_libraries
                .effective_definition_libraries(name, kind, &["shared"])
                .as_slice(),
            [EffectiveDefinitionLibrary::Unobserved(candidate)]
                if candidate.id() == library.id()
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
        .build(&db)
        .expect("partially known library project fixture should build");

    let libraries = template_library_catalog(&db, project);
    assert!(matches!(
        project_inventory(libraries).loadable_library_str("shared"),
        LoadableLibraryLookup::Found(library)
            if library.module_name_str() == "project_tags"
    ));
    assert_eq!(
        project_inventory(libraries).loadable_library_str("other"),
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
        .build(&db)
        .expect("correlated-library project fixture should build");
    let inventory = template_library_catalog(&db, project);
    let scoped_libraries = ScopedTemplateLibraries::from_project_inventory(inventory);

    let definitions = scoped_libraries.effective_definition_libraries(
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
fn loadable_duplicate_load_name_uses_last_record() {
    let libraries = libraries(
        false,
        vec![
            loadable("custom", "project.templatetags.original", Vec::new())
                .expect("loadable library fixture should be valid"),
            loadable("custom", "project.templatetags.replacement", Vec::new())
                .expect("loadable library fixture should be valid"),
        ],
    );
    let library = project_inventory(&libraries)
        .loadable_library(&library_name("custom").expect("test library name should be valid"))
        .found()
        .expect("duplicate custom load name should resolve to one definitive library");
    assert_eq!(
        library.module_name_str(),
        "project.templatetags.replacement"
    );
}

#[test]
fn available_symbol_guidance_survives_open_remainder() {
    let app = module("available_in_app").expect("test Python module name should be valid");
    let load_name = library_name("extra_tags").expect("test library name should be valid");
    let libraries = libraries(
        true,
        vec![
            available_in_app(
                "extra_tags",
                "available_in_app",
                "available_in_app.templatetags.extra_tags",
                vec![symbol(TemplateSymbolKind::Tag, "extra", None)],
            )
            .expect("available-in-app library fixture should be valid"),
        ],
    );
    assert_eq!(
        project_inventory(&libraries).available_in_app_symbol("extra", TemplateSymbolKind::Tag),
        AppTemplateSymbolLookup::FoundInApp { app, load_name }
    );
}

#[test]
fn available_in_app_library_guidance_is_sorted_and_deduplicated() {
    let libraries = libraries(
        false,
        vec![
            available_in_app("shared", "beta", "beta.templatetags.shared", Vec::new())
                .expect("available-in-app library fixture should be valid"),
            available_in_app(
                "shared",
                "alpha",
                "alpha.templatetags.shared_extra",
                Vec::new(),
            )
            .expect("available-in-app library fixture should be valid"),
            available_in_app("shared", "alpha", "alpha.templatetags.shared", Vec::new())
                .expect("available-in-app library fixture should be valid"),
        ],
    );
    let apps = available_in_app_candidates(
        project_inventory(&libraries)
            .missing_library(&library_name("shared").expect("test library name should be valid")),
    )
    .expect("missing shared library should have available-in-app candidates");
    assert_eq!(
        apps.primary(),
        &module("alpha").expect("test Python module name should be valid")
    );
    assert_eq!(
        apps.as_slice(),
        [
            module("alpha").expect("test Python module name should be valid"),
            module("beta").expect("test Python module name should be valid")
        ]
    );
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
            )
            .expect("builtin library fixture should be valid"),
            builtin(
                "a_second",
                vec![symbol(
                    TemplateSymbolKind::Filter,
                    "duplicate",
                    Some("second"),
                )],
            )
            .expect("builtin library fixture should be valid"),
            loadable(
                "humanize",
                "django.contrib.humanize.templatetags.humanize",
                vec![symbol(TemplateSymbolKind::Filter, "intcomma", None)],
            )
            .expect("loadable library fixture should be valid"),
        ],
    );
    let scoped_libraries = project_inventory(&libraries);
    let candidates: Vec<_> = scoped_libraries
        .inventory_symbol_names(TemplateSymbolKind::Filter)
        .flat_map(|name| {
            scoped_libraries.scoped_symbol_candidates(name, TemplateSymbolKind::Filter)
        })
        .collect();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].symbol.name(), "duplicate");
    let duplicate_library = scoped_libraries
        .resolved_libraries()
        .into_iter()
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
    let libraries = testing::template_library_catalog(
        &db,
        vec![
            builtin("django.template.defaulttags", Vec::new())
                .expect("builtin library fixture should be valid"),
            builtin("project.builtins", Vec::new())
                .expect("builtin library fixture should be valid"),
            builtin("django.template.defaulttags", Vec::new())
                .expect("builtin library fixture should be valid"),
        ],
    );
    let scoped_libraries = project_inventory(&libraries);
    let modules: Vec<_> = scoped_libraries
        .resolved_libraries()
        .into_iter()
        .map(djls_project::TemplateLibrary::module_name_str)
        .collect();
    assert_eq!(
        modules,
        vec!["django.template.defaulttags", "project.builtins"]
    );
    assert!(
        scoped_libraries
            .resolved_libraries()
            .into_iter()
            .all(|library| library.source_file().is_none()),
        "configured test evidence must not invent source origins"
    );
}

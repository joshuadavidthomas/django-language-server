use djls_project::LibraryName;
use djls_project::LoadableLibraryLookup;
use djls_project::MissingLibraryLookup;
use djls_project::PythonModuleName;
use djls_project::SymbolDefinition;
use djls_project::TemplateLibraries;
use djls_project::TemplateSymbol;
use djls_project::TemplateSymbolAvailability;
use djls_project::TemplateSymbolKind;
use djls_project::TemplateSymbolLookup;
use djls_project::TemplateSymbolName;
use djls_project::testing;
use djls_project::testing::TemplateBackendLibrariesInput;
use djls_project::testing::TemplateLibraryConfigurationInput;
use djls_project::testing::TemplateLibraryInput;
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
    testing::template_libraries(&TestDatabase::new(), open, inputs)
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
    testing::template_libraries_with_configurations(
        &db,
        inputs,
        configurations
            .into_iter()
            .map(|backends| TemplateLibraryConfigurationInput { backends })
            .collect(),
        open,
    )
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
    assert_eq!(
        libraries.missing_library_lookup(&library_name("shared")),
        MissingLibraryLookup::FoundInApps {
            primary_app: module("alpha"),
            apps: vec![module("alpha"), module("beta")],
        }
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
fn resolved_libraries_retain_duplicate_builtins_in_order() {
    let db = TestDatabase::new();
    let libraries = testing::template_libraries(
        &db,
        false,
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
        vec![
            "django.template.defaulttags",
            "project.builtins",
            "django.template.defaulttags",
        ]
    );
    assert!(libraries.resolved_libraries().all(|library| {
        library
            .file()
            .path(&db)
            .as_str()
            .starts_with("/__djls_testing__/")
    }));
}

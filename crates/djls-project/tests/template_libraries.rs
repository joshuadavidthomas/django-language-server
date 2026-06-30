use djls_project::LibraryName;
use djls_project::PythonModuleName;
use djls_project::SymbolDefinition;
use djls_project::TemplateLibraries;
use djls_project::TemplateSymbol;
use djls_project::TemplateSymbolAvailability;
use djls_project::TemplateSymbolKind;
use djls_project::TemplateSymbolName;
use djls_project::UnknownLibraryOutcome;
use djls_project::UnknownSymbolOutcome;
use djls_project::testing;
use djls_project::testing::StaticKnowledge;
use djls_project::testing::TemplateLibraryInput;
use djls_testing::TestDatabase;

fn module(name: &str) -> PythonModuleName {
    PythonModuleName::parse(name).unwrap()
}

fn symbol(kind: TemplateSymbolKind, name: &str, doc: Option<&str>) -> TemplateSymbol {
    TemplateSymbol {
        kind,
        name: TemplateSymbolName::parse(name).unwrap(),
        definition: SymbolDefinition::Unknown,
        doc: doc.map(str::to_string),
    }
}

fn library_name(name: &str) -> LibraryName {
    LibraryName::parse(name).unwrap()
}

fn libraries(
    knowledge: StaticKnowledge,
    builtins: Vec<TemplateLibraryInput>,
    installed: Vec<TemplateLibraryInput>,
    available: Vec<TemplateLibraryInput>,
) -> TemplateLibraries {
    let db = TestDatabase::new();
    let inputs = builtins
        .into_iter()
        .chain(installed)
        .chain(available)
        .collect();
    testing::template_libraries(&db, knowledge, inputs)
}

fn empty_libraries(knowledge: StaticKnowledge) -> TemplateLibraries {
    libraries(knowledge, Vec::new(), Vec::new(), Vec::new())
}

fn builtin(module: PythonModuleName, symbols: Vec<TemplateSymbol>) -> TemplateLibraryInput {
    TemplateLibraryInput::Builtin { module, symbols }
}

fn installed(
    load_name: LibraryName,
    module: PythonModuleName,
    symbols: Vec<TemplateSymbol>,
) -> TemplateLibraryInput {
    TemplateLibraryInput::Installed {
        load_name,
        module,
        symbols,
    }
}

fn available(
    load_name: LibraryName,
    app: PythonModuleName,
    module: PythonModuleName,
    symbols: Vec<TemplateSymbol>,
) -> TemplateLibraryInput {
    TemplateLibraryInput::Available {
        load_name,
        app,
        module,
        symbols,
    }
}

#[test]
fn unknown_tag_outcome_suppresses_incomplete_inventory() {
    let libraries = empty_libraries(StaticKnowledge::Partial);

    assert_eq!(
        libraries.unknown_tag_outcome("missing"),
        UnknownSymbolOutcome::Suppressed
    );
}

#[test]
fn unknown_tag_outcome_reports_available_app_candidate() {
    let app = module("available_app");
    let load_name = library_name("extra_tags");
    let libraries = libraries(
        StaticKnowledge::Known,
        Vec::new(),
        Vec::new(),
        vec![available(
            load_name.clone(),
            app.clone(),
            module("available_app.templatetags.extra_tags"),
            vec![symbol(TemplateSymbolKind::Tag, "extra", None)],
        )],
    );

    assert_eq!(
        libraries.unknown_tag_outcome("extra"),
        UnknownSymbolOutcome::Available { app, load_name }
    );
}

#[test]
fn unknown_tag_outcome_reports_truly_unknown() {
    let libraries = empty_libraries(StaticKnowledge::Known);

    assert_eq!(
        libraries.unknown_tag_outcome("missing"),
        UnknownSymbolOutcome::TrulyUnknown
    );
}

#[test]
fn unknown_filter_outcome_suppresses_incomplete_inventory() {
    let libraries = empty_libraries(StaticKnowledge::Partial);

    assert_eq!(
        libraries.unknown_filter_outcome("missing"),
        UnknownSymbolOutcome::Suppressed
    );
}

#[test]
fn unknown_filter_outcome_reports_available_app_candidate() {
    let app = module("available_app");
    let load_name = library_name("extra_filters");
    let libraries = libraries(
        StaticKnowledge::Known,
        Vec::new(),
        Vec::new(),
        vec![available(
            load_name.clone(),
            app.clone(),
            module("available_app.templatetags.extra_filters"),
            vec![symbol(TemplateSymbolKind::Filter, "extra", None)],
        )],
    );

    assert_eq!(
        libraries.unknown_filter_outcome("extra"),
        UnknownSymbolOutcome::Available { app, load_name }
    );
}

#[test]
fn unknown_filter_outcome_reports_truly_unknown() {
    let libraries = empty_libraries(StaticKnowledge::Known);

    assert_eq!(
        libraries.unknown_filter_outcome("missing"),
        UnknownSymbolOutcome::TrulyUnknown
    );
}

#[test]
fn unknown_library_outcome_suppresses_incomplete_inventory() {
    let libraries = empty_libraries(StaticKnowledge::Partial);

    assert_eq!(
        libraries.unknown_library_outcome(&library_name("missing")),
        UnknownLibraryOutcome::Suppressed
    );
}

#[test]
fn unknown_library_outcome_reports_available_app_candidates() {
    let shared = library_name("shared");
    let alpha = module("alpha");
    let beta = module("beta");
    let libraries = libraries(
        StaticKnowledge::Known,
        Vec::new(),
        Vec::new(),
        vec![
            available(
                shared.clone(),
                beta.clone(),
                module("beta.templatetags.shared"),
                Vec::new(),
            ),
            available(
                shared.clone(),
                alpha.clone(),
                module("alpha.templatetags.shared_extra"),
                Vec::new(),
            ),
            available(
                shared.clone(),
                alpha.clone(),
                module("alpha.templatetags.shared"),
                Vec::new(),
            ),
        ],
    );

    assert_eq!(
        libraries.unknown_library_outcome(&shared),
        UnknownLibraryOutcome::AvailableInApps {
            primary_app: alpha.clone(),
            apps: vec![alpha, beta],
        }
    );
}

#[test]
fn unknown_library_outcome_reports_truly_unknown() {
    let libraries = empty_libraries(StaticKnowledge::Known);

    assert_eq!(
        libraries.unknown_library_outcome(&library_name("missing")),
        UnknownLibraryOutcome::TrulyUnknown
    );
}

#[test]
fn installed_library_count_counts_builtins_and_installed() {
    let libraries = libraries(
        StaticKnowledge::Known,
        vec![
            builtin(module("django.template.defaulttags"), Vec::new()),
            builtin(module("project.builtins"), Vec::new()),
        ],
        vec![installed(
            library_name("custom"),
            module("project.templatetags.custom"),
            Vec::new(),
        )],
        vec![available(
            library_name("available"),
            module("available_app"),
            module("available_app.templatetags.available"),
            Vec::new(),
        )],
    );

    assert_eq!(libraries.installed_library_count(), 3);
}

#[test]
fn installed_libraries_replace_duplicate_load_names() {
    let load_name = library_name("custom");
    let replacement_module = module("project.templatetags.replacement");
    let libraries = libraries(
        StaticKnowledge::Known,
        Vec::new(),
        vec![
            installed(
                load_name.clone(),
                module("project.templatetags.original"),
                vec![symbol(TemplateSymbolKind::Tag, "original", None)],
            ),
            installed(
                load_name.clone(),
                replacement_module.clone(),
                vec![symbol(TemplateSymbolKind::Tag, "replacement", None)],
            ),
        ],
        Vec::new(),
    );

    let library = libraries
        .installed_library(&load_name)
        .expect("replacement library should be installed");
    assert_eq!(library.module_name(), &replacement_module);
    assert_eq!(libraries.completion_library_names(), vec![load_name]);

    let symbols: Vec<_> = libraries
        .template_symbol_candidates(TemplateSymbolKind::Tag)
        .into_iter()
        .map(|candidate| candidate.symbol.name().to_string())
        .collect();
    assert_eq!(symbols, vec!["replacement"]);
}

#[test]
fn available_libraries_dedup_by_app_and_module() {
    let app = module("available_app");
    let load_name = library_name("extra_tags");
    let module_name = module("available_app.templatetags.extra_tags");
    let libraries = libraries(
        StaticKnowledge::Known,
        Vec::new(),
        Vec::new(),
        vec![
            available(
                load_name.clone(),
                app.clone(),
                module_name.clone(),
                vec![symbol(TemplateSymbolKind::Tag, "first", None)],
            ),
            available(
                load_name.clone(),
                app.clone(),
                module_name,
                vec![symbol(TemplateSymbolKind::Tag, "second", None)],
            ),
        ],
    );

    assert_eq!(
        libraries.unknown_tag_outcome("first"),
        UnknownSymbolOutcome::Available {
            app: app.clone(),
            load_name: load_name.clone(),
        }
    );
    assert_eq!(
        libraries.unknown_tag_outcome("second"),
        UnknownSymbolOutcome::TrulyUnknown
    );
    assert_eq!(
        libraries.unknown_library_outcome(&load_name),
        UnknownLibraryOutcome::AvailableInApps {
            primary_app: app.clone(),
            apps: vec![app],
        }
    );
}

#[test]
fn template_symbol_candidates_keep_last_builtin_symbol() {
    let a_second = module("a_second");
    let libraries = libraries(
        StaticKnowledge::Known,
        vec![
            builtin(
                module("z_first"),
                vec![symbol(
                    TemplateSymbolKind::Filter,
                    "duplicate",
                    Some("first"),
                )],
            ),
            builtin(
                a_second.clone(),
                vec![symbol(
                    TemplateSymbolKind::Filter,
                    "duplicate",
                    Some("second"),
                )],
            ),
        ],
        Vec::new(),
        Vec::new(),
    );

    let candidates = libraries.template_symbol_candidates(TemplateSymbolKind::Filter);

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].symbol.doc.as_deref(), Some("second"));
    assert!(matches!(
        &candidates[0].availability,
        TemplateSymbolAvailability::Builtin { module } if module == &a_second
    ));
}

#[test]
fn template_symbol_candidates_report_load_requirement() {
    let load_name = library_name("humanize");
    let libraries = libraries(
        StaticKnowledge::Known,
        Vec::new(),
        vec![installed(
            load_name.clone(),
            module("django.contrib.humanize.templatetags.humanize"),
            vec![symbol(TemplateSymbolKind::Filter, "intcomma", None)],
        )],
        Vec::new(),
    );

    let candidates = libraries.template_symbol_candidates(TemplateSymbolKind::Filter);

    assert_eq!(candidates.len(), 1);
    assert!(matches!(
        &candidates[0].availability,
        TemplateSymbolAvailability::RequiresLoad { load_name: candidate_load_name }
            if candidate_load_name == &load_name
    ));
}

#[test]
fn testing_inventory_files_belong_to_supplied_db() {
    let db = TestDatabase::new();
    let libraries = testing::template_libraries(
        &db,
        StaticKnowledge::Known,
        vec![builtin(module("project.templatetags.custom"), Vec::new())],
    );

    let paths: Vec<_> = libraries
        .active_libraries()
        .map(|library| library.file().path(&db).to_string())
        .collect();

    assert_eq!(
        paths,
        vec!["/__djls_testing__/project/templatetags/custom.py"]
    );
}

#[test]
fn builtin_libraries_retain_duplicate_modules_in_order() {
    let libraries = libraries(
        StaticKnowledge::Known,
        vec![
            builtin(module("django.template.defaulttags"), Vec::new()),
            builtin(module("project.builtins"), Vec::new()),
            builtin(module("django.template.defaulttags"), Vec::new()),
        ],
        Vec::new(),
        Vec::new(),
    );

    let modules: Vec<_> = libraries
        .active_libraries()
        .map(|library| library.module_name().as_str().to_string())
        .collect();

    assert_eq!(
        modules,
        vec![
            "django.template.defaulttags",
            "project.builtins",
            "django.template.defaulttags",
        ]
    );
}

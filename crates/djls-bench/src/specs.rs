use std::sync::OnceLock;

use djls_project::FilterArity;
use djls_project::FilterArityMap;
use djls_project::LibraryName;
use djls_project::ModulePath;
use djls_project::PyModuleName;
use djls_project::SymbolDefinition;
use djls_project::SymbolKey;
use djls_project::TemplateLibraries;
use djls_project::TemplateLibrary;
use djls_project::TemplateSymbol;
use djls_project::TemplateSymbolKind;
use djls_project::TemplateSymbolName;
use djls_semantic::FilterAritySpecs;
use djls_semantic::TagSpecs;
use djls_testing::extract_bundle;

use crate::Db;

const DEFAULTTAGS: &str = "django.template.defaulttags";
const DEFAULTFILTERS: &str = "django.template.defaultfilters";
const I18N: &str = "django.templatetags.i18n";
const STATIC: &str = "django.templatetags.static";

struct BenchSymbol {
    load_name: Option<&'static str>,
    module: &'static str,
    symbol: TemplateSymbol,
}

fn template_symbol(kind: TemplateSymbolKind, name: &str, module: &str) -> TemplateSymbol {
    TemplateSymbol {
        kind,
        name: TemplateSymbolName::parse(name).unwrap(),
        definition: PyModuleName::parse(module)
            .map_or(SymbolDefinition::Unknown, SymbolDefinition::Module),
        doc: None,
    }
}

fn builtin_tag(name: &str, module: &'static str) -> BenchSymbol {
    BenchSymbol {
        load_name: None,
        module,
        symbol: template_symbol(TemplateSymbolKind::Tag, name, module),
    }
}

fn library_tag(name: &str, load_name: &'static str, module: &'static str) -> BenchSymbol {
    BenchSymbol {
        load_name: Some(load_name),
        module,
        symbol: template_symbol(TemplateSymbolKind::Tag, name, module),
    }
}

fn builtin_filter(name: &str, module: &'static str) -> BenchSymbol {
    BenchSymbol {
        load_name: None,
        module,
        symbol: template_symbol(TemplateSymbolKind::Filter, name, module),
    }
}

struct RealisticSpecs {
    tag_specs: TagSpecs,
    template_libraries: TemplateLibraries,
    filter_arity_specs: FilterAritySpecs,
}

fn build_template_symbols() -> Vec<BenchSymbol> {
    vec![
        builtin_tag("if", DEFAULTTAGS),
        builtin_tag("for", DEFAULTTAGS),
        builtin_tag("block", DEFAULTTAGS),
        builtin_tag("extends", DEFAULTTAGS),
        builtin_tag("include", DEFAULTTAGS),
        builtin_tag("with", DEFAULTTAGS),
        builtin_tag("load", DEFAULTTAGS),
        builtin_tag("url", DEFAULTTAGS),
        builtin_tag("csrf_token", DEFAULTTAGS),
        builtin_tag("comment", DEFAULTTAGS),
        builtin_tag("verbatim", DEFAULTTAGS),
        builtin_tag("autoescape", DEFAULTTAGS),
        builtin_tag("spaceless", DEFAULTTAGS),
        builtin_tag("widthratio", DEFAULTTAGS),
        builtin_tag("cycle", DEFAULTTAGS),
        builtin_tag("firstof", DEFAULTTAGS),
        builtin_tag("now", DEFAULTTAGS),
        builtin_tag("regroup", DEFAULTTAGS),
        builtin_tag("ifchanged", DEFAULTTAGS),
        builtin_tag("filter", DEFAULTTAGS),
        builtin_filter("title", DEFAULTFILTERS),
        builtin_filter("lower", DEFAULTFILTERS),
        builtin_filter("upper", DEFAULTFILTERS),
        builtin_filter("default", DEFAULTFILTERS),
        builtin_filter("date", DEFAULTFILTERS),
        builtin_filter("truncatewords", DEFAULTFILTERS),
        builtin_filter("floatformat", DEFAULTFILTERS),
        builtin_filter("length", DEFAULTFILTERS),
        builtin_filter("join", DEFAULTFILTERS),
        builtin_filter("safe", DEFAULTFILTERS),
        builtin_filter("escape", DEFAULTFILTERS),
        builtin_filter("urlencode", DEFAULTFILTERS),
        builtin_filter("slugify", DEFAULTFILTERS),
        builtin_filter("linebreaks", DEFAULTFILTERS),
        builtin_filter("striptags", DEFAULTFILTERS),
        builtin_filter("capfirst", DEFAULTFILTERS),
        builtin_filter("center", DEFAULTFILTERS),
        builtin_filter("cut", DEFAULTFILTERS),
        builtin_filter("dictsort", DEFAULTFILTERS),
        builtin_filter("yesno", DEFAULTFILTERS),
        builtin_filter("pluralize", DEFAULTFILTERS),
        library_tag("translate", "i18n", I18N),
        library_tag("trans", "i18n", I18N),
        library_tag("blocktranslate", "i18n", I18N),
        library_tag("blocktrans", "i18n", I18N),
        library_tag("get_current_language", "i18n", I18N),
        library_tag("static", "static", STATIC),
    ]
}

fn build_filter_arities(
    defaultfilters: &str,
    extracted_arities: &[&FilterArityMap],
) -> FilterAritySpecs {
    let mut specs = FilterAritySpecs::new();
    for filter_arities in extracted_arities {
        specs.merge_filter_arities(filter_arities);
    }

    let known: &[(&str, bool, bool)] = &[
        ("title", false, false),
        ("lower", false, false),
        ("upper", false, false),
        ("default", true, false),
        ("date", true, true),
        ("truncatewords", true, false),
        ("floatformat", true, true),
        ("join", true, false),
        ("cut", true, false),
        ("yesno", true, true),
        ("pluralize", true, true),
        ("center", true, false),
    ];

    for &(name, expects_arg, arg_optional) in known {
        specs.insert(
            SymbolKey::filter(defaultfilters, name),
            FilterArity {
                expects_arg,
                arg_optional,
            },
        );
    }

    specs
}

fn build_template_libraries(symbols: Vec<BenchSymbol>) -> TemplateLibraries {
    let mut libraries = TemplateLibraries {
        knowledge: djls_project::StaticKnowledge::Known,
        ..TemplateLibraries::default()
    };

    for module_name in [DEFAULTTAGS, DEFAULTFILTERS] {
        let module = PyModuleName::parse(module_name).unwrap();
        libraries.builtins.push(TemplateLibrary::new(module));
    }

    for (load_name, module_name) in [("i18n", I18N), ("static", STATIC)] {
        let load_name = LibraryName::parse(load_name).unwrap();
        let module = PyModuleName::parse(module_name).unwrap();
        libraries
            .loadable
            .insert(load_name, TemplateLibrary::new(module));
    }

    for bench_symbol in symbols {
        match bench_symbol.load_name {
            None => {
                let module = PyModuleName::parse(bench_symbol.module).unwrap();
                if let Some(library) = libraries
                    .builtins
                    .iter_mut()
                    .find(|library| library.module() == &module)
                {
                    library.merge_symbol(bench_symbol.symbol);
                }
            }
            Some(load_name) => {
                let load_name = LibraryName::parse(load_name).unwrap();
                let module = PyModuleName::parse(bench_symbol.module).unwrap();
                if let Some(library) = libraries.loadable.get_mut(&load_name)
                    && library.module() == &module
                {
                    library.merge_symbol(bench_symbol.symbol);
                }
            }
        }
    }

    libraries
}

fn build_realistic_specs() -> RealisticSpecs {
    let template_libraries = build_template_libraries(build_template_symbols());

    let mut tag_specs = TagSpecs::default();

    let fixture_root = crate::fixtures::crate_root().join("fixtures/python");
    let defaulttags_path = fixture_root.join("large/defaulttags.py");
    let defaulttags_source = std::fs::read_to_string(&defaulttags_path)
        .unwrap_or_else(|err| panic!("failed to load defaulttags.py fixture: {err}"));
    let i18n_path = fixture_root.join("medium/i18n.py");
    let i18n_source = std::fs::read_to_string(&i18n_path)
        .unwrap_or_else(|err| panic!("failed to load i18n.py fixture: {err}"));

    let mut extraction_db = Db::new();
    let defaulttags_file = extraction_db.file_with_contents(defaulttags_path, &defaulttags_source);
    let i18n_file = extraction_db.file_with_contents(i18n_path, &i18n_source);

    let defaulttags = extract_bundle(
        &extraction_db,
        defaulttags_file,
        ModulePath::new(DEFAULTTAGS),
    );
    tag_specs
        .merge_block_specs(&defaulttags.block_specs)
        .merge_tag_rules(&defaulttags.tag_rules);

    let i18n = extract_bundle(&extraction_db, i18n_file, ModulePath::new(I18N));
    tag_specs
        .merge_block_specs(&i18n.block_specs)
        .merge_tag_rules(&i18n.tag_rules);

    let filter_arity_specs = build_filter_arities(
        DEFAULTFILTERS,
        &[&defaulttags.filter_arities, &i18n.filter_arities],
    );

    RealisticSpecs {
        tag_specs,
        template_libraries,
        filter_arity_specs,
    }
}

/// Create a benchmark `Db` configured with realistic Django tag specs,
/// template libraries, and filter arity data extracted from real Django
/// source files.
pub fn realistic_db() -> Db {
    static SPECS: OnceLock<RealisticSpecs> = OnceLock::new();
    let specs = SPECS.get_or_init(build_realistic_specs);

    Db::new()
        .with_tag_specs(specs.tag_specs.clone())
        .with_template_libraries(specs.template_libraries.clone())
        .with_filter_arity_specs(specs.filter_arity_specs.clone())
}

use std::collections::HashMap;
use std::sync::OnceLock;

use djls_project::Db as ProjectDb;
use djls_project::FilterArity;
use djls_project::FilterArityMap;
use djls_project::PythonModulePath;
use djls_project::SymbolDefinition;
use djls_project::SymbolKey;
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
        definition: PythonModulePath::parse(module)
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

fn build_template_libraries(
    db: &dyn ProjectDb,
    symbols: Vec<BenchSymbol>,
) -> djls_project::TemplateLibraries {
    let mut tags = Vec::new();
    let mut filters = Vec::new();

    for bench_symbol in symbols {
        let symbol_name = bench_symbol.symbol.name();
        let value = match (bench_symbol.symbol.kind, bench_symbol.load_name) {
            (TemplateSymbolKind::Tag, None) => {
                djls_testing::builtin_tag(symbol_name, bench_symbol.module)
            }
            (TemplateSymbolKind::Tag, Some(load_name)) => {
                djls_testing::library_tag(symbol_name, load_name, bench_symbol.module)
            }
            (TemplateSymbolKind::Filter, None) => {
                djls_testing::builtin_filter(symbol_name, bench_symbol.module)
            }
            (TemplateSymbolKind::Filter, Some(load_name)) => {
                djls_testing::library_filter(symbol_name, load_name, bench_symbol.module)
            }
        };

        match bench_symbol.symbol.kind {
            TemplateSymbolKind::Tag => tags.push(value),
            TemplateSymbolKind::Filter => filters.push(value),
        }
    }

    let libraries = HashMap::from([
        ("i18n".to_string(), I18N.to_string()),
        ("static".to_string(), STATIC.to_string()),
    ]);
    let builtins = vec![DEFAULTTAGS.to_string(), DEFAULTFILTERS.to_string()];

    djls_testing::make_template_libraries(db, &tags, &filters, &libraries, &builtins)
}

fn build_realistic_specs() -> RealisticSpecs {
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
        PythonModulePath::parse(DEFAULTTAGS).unwrap(),
    );
    tag_specs
        .merge_block_specs(&defaulttags.block_specs)
        .merge_tag_rules(&defaulttags.tag_rules);

    let i18n = extract_bundle(
        &extraction_db,
        i18n_file,
        PythonModulePath::parse(I18N).unwrap(),
    );
    tag_specs
        .merge_block_specs(&i18n.block_specs)
        .merge_tag_rules(&i18n.tag_rules);

    let filter_arity_specs = build_filter_arities(
        DEFAULTFILTERS,
        &[&defaulttags.filter_arities, &i18n.filter_arities],
    );

    RealisticSpecs {
        tag_specs,
        filter_arity_specs,
    }
}

/// Create a benchmark `Db` configured with realistic Django tag specs,
/// template libraries, and filter arity data extracted from real Django
/// source files.
pub fn realistic_db() -> Db {
    static SPECS: OnceLock<RealisticSpecs> = OnceLock::new();
    let specs = SPECS.get_or_init(build_realistic_specs);

    let db = Db::new()
        .with_tag_specs(specs.tag_specs.clone())
        .with_filter_arity_specs(specs.filter_arity_specs.clone());
    let template_libraries = build_template_libraries(&db, build_template_symbols());
    db.with_template_libraries(template_libraries)
}

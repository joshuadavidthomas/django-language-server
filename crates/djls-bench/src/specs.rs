use std::collections::BTreeMap;
use std::sync::OnceLock;

use djls_project::InspectorLibrarySymbol;
use djls_project::TemplateLibraries;
use djls_project::TemplateLibrariesResponse;
use djls_project::TemplateSymbolKind;
use djls_python::FilterArity;
use djls_python::SymbolKey;
use djls_semantic::FilterAritySpecs;
use djls_semantic::TagSpecs;

use crate::Db;

const DEFAULTTAGS: &str = "django.template.defaulttags";
const DEFAULTFILTERS: &str = "django.template.defaultfilters";
const I18N: &str = "django.templatetags.i18n";
const STATIC: &str = "django.templatetags.static";

fn builtin_tag(name: &str, module: &str) -> InspectorLibrarySymbol {
    InspectorLibrarySymbol {
        kind: Some(TemplateSymbolKind::Tag),
        name: name.to_string(),
        load_name: None,
        library_module: module.to_string(),
        module: module.to_string(),
        doc: None,
    }
}

fn library_tag(name: &str, load_name: &str, module: &str) -> InspectorLibrarySymbol {
    InspectorLibrarySymbol {
        kind: Some(TemplateSymbolKind::Tag),
        name: name.to_string(),
        load_name: Some(load_name.to_string()),
        library_module: module.to_string(),
        module: module.to_string(),
        doc: None,
    }
}

fn builtin_filter(name: &str, module: &str) -> InspectorLibrarySymbol {
    InspectorLibrarySymbol {
        kind: Some(TemplateSymbolKind::Filter),
        name: name.to_string(),
        load_name: None,
        library_module: module.to_string(),
        module: module.to_string(),
        doc: None,
    }
}

struct RealisticSpecs {
    tag_specs: TagSpecs,
    template_libraries: TemplateLibraries,
    filter_arity_specs: FilterAritySpecs,
}

fn build_inspector_symbols() -> Vec<InspectorLibrarySymbol> {
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
    extraction: &djls_python::ExtractionResult,
) -> FilterAritySpecs {
    let mut specs = FilterAritySpecs::new();
    specs.merge_extraction_result(extraction);

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

fn build_realistic_specs() -> RealisticSpecs {
    let symbols = build_inspector_symbols();

    let mut libraries_map = BTreeMap::new();
    libraries_map.insert("i18n".to_string(), I18N.to_string());
    libraries_map.insert("static".to_string(), STATIC.to_string());

    let builtins = vec![DEFAULTTAGS.to_string(), DEFAULTFILTERS.to_string()];

    let response = TemplateLibrariesResponse {
        symbols,
        libraries: libraries_map,
        builtins,
    };

    let template_libraries = TemplateLibraries::default().apply_inspector(Some(response));

    let mut tag_specs = TagSpecs::default();

    let fixture_root = crate::fixtures::crate_root().join("fixtures/python");
    let defaulttags_source = std::fs::read_to_string(fixture_root.join("large/defaulttags.py"))
        .unwrap_or_else(|err| panic!("failed to load defaulttags.py fixture: {err}"));

    let mut extraction = djls_python::extract_rules(&defaulttags_source, DEFAULTTAGS);
    tag_specs.merge_extraction_results(&extraction);

    let i18n_source = std::fs::read_to_string(fixture_root.join("medium/i18n.py"))
        .unwrap_or_else(|err| panic!("failed to load i18n.py fixture: {err}"));
    let i18n_extraction = djls_python::extract_rules(&i18n_source, I18N);
    tag_specs.merge_extraction_results(&i18n_extraction);
    extraction.merge(i18n_extraction);

    let filter_arity_specs = build_filter_arities(DEFAULTFILTERS, &extraction);

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

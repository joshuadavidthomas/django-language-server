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
    let defaulttags = "django.template.defaulttags";
    let defaultfilters = "django.template.defaultfilters";
    let i18n = "django.templatetags.i18n";
    let static_mod = "django.templatetags.static";

    vec![
        builtin_tag("if", defaulttags),
        builtin_tag("for", defaulttags),
        builtin_tag("block", defaulttags),
        builtin_tag("extends", defaulttags),
        builtin_tag("include", defaulttags),
        builtin_tag("with", defaulttags),
        builtin_tag("load", defaulttags),
        builtin_tag("url", defaulttags),
        builtin_tag("csrf_token", defaulttags),
        builtin_tag("comment", defaulttags),
        builtin_tag("verbatim", defaulttags),
        builtin_tag("autoescape", defaulttags),
        builtin_tag("spaceless", defaulttags),
        builtin_tag("widthratio", defaulttags),
        builtin_tag("cycle", defaulttags),
        builtin_tag("firstof", defaulttags),
        builtin_tag("now", defaulttags),
        builtin_tag("regroup", defaulttags),
        builtin_tag("ifchanged", defaulttags),
        builtin_tag("filter", defaulttags),
        builtin_filter("title", defaultfilters),
        builtin_filter("lower", defaultfilters),
        builtin_filter("upper", defaultfilters),
        builtin_filter("default", defaultfilters),
        builtin_filter("date", defaultfilters),
        builtin_filter("truncatewords", defaultfilters),
        builtin_filter("floatformat", defaultfilters),
        builtin_filter("length", defaultfilters),
        builtin_filter("join", defaultfilters),
        builtin_filter("safe", defaultfilters),
        builtin_filter("escape", defaultfilters),
        builtin_filter("urlencode", defaultfilters),
        builtin_filter("slugify", defaultfilters),
        builtin_filter("linebreaks", defaultfilters),
        builtin_filter("striptags", defaultfilters),
        builtin_filter("capfirst", defaultfilters),
        builtin_filter("center", defaultfilters),
        builtin_filter("cut", defaultfilters),
        builtin_filter("dictsort", defaultfilters),
        builtin_filter("yesno", defaultfilters),
        builtin_filter("pluralize", defaultfilters),
        library_tag("translate", "i18n", i18n),
        library_tag("trans", "i18n", i18n),
        library_tag("blocktranslate", "i18n", i18n),
        library_tag("blocktrans", "i18n", i18n),
        library_tag("get_current_language", "i18n", i18n),
        library_tag("static", "static", static_mod),
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
    let defaulttags = "django.template.defaulttags";
    let defaultfilters = "django.template.defaultfilters";
    let i18n = "django.templatetags.i18n";
    let static_mod = "django.templatetags.static";

    let symbols = build_inspector_symbols();

    let mut libraries_map = BTreeMap::new();
    libraries_map.insert("i18n".to_string(), i18n.to_string());
    libraries_map.insert("static".to_string(), static_mod.to_string());

    let builtins = vec![defaulttags.to_string(), defaultfilters.to_string()];

    let response = TemplateLibrariesResponse {
        symbols,
        libraries: libraries_map,
        builtins,
    };

    let template_libraries = TemplateLibraries::default().apply_inspector(Some(response));

    let mut tag_specs = TagSpecs::default();

    let fixture_root = crate::fixtures::crate_root().join("fixtures/python");
    let defaulttags_source =
        std::fs::read_to_string(fixture_root.join("large/defaulttags.py")).unwrap_or_default();

    if defaulttags_source.is_empty() {
        return RealisticSpecs {
            tag_specs,
            template_libraries,
            filter_arity_specs: FilterAritySpecs::new(),
        };
    }

    let mut extraction = djls_python::extract_rules(&defaulttags_source, defaulttags);
    tag_specs.merge_extraction_results(&extraction);

    let i18n_source =
        std::fs::read_to_string(fixture_root.join("medium/i18n.py")).unwrap_or_default();
    if !i18n_source.is_empty() {
        let i18n_extraction = djls_python::extract_rules(&i18n_source, i18n);
        tag_specs.merge_extraction_results(&i18n_extraction);
        extraction.merge(i18n_extraction);
    }

    let filter_arity_specs = build_filter_arities(defaultfilters, &extraction);

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

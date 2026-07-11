use std::sync::OnceLock;

use djls_project::FilterArity;
use djls_project::FilterArityMap;
use djls_project::PythonModuleName;
use djls_project::SymbolKey;
use djls_semantic::FilterAritySpecs;
use djls_semantic::TagSpecs;
use djls_testing::extract_bundle;

use crate::Db;

const DEFAULTTAGS: &str = "django.template.defaulttags";
const DEFAULTFILTERS: &str = "django.template.defaultfilters";
const I18N: &str = "django.templatetags.i18n";
struct RealisticSpecs {
    tag_specs: TagSpecs,
    filter_arity_specs: FilterAritySpecs,
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
        PythonModuleName::parse(DEFAULTTAGS).unwrap(),
    );
    tag_specs
        .merge_block_specs(&defaulttags.block_specs)
        .merge_tag_rules(&defaulttags.tag_rules);

    let i18n = extract_bundle(
        &extraction_db,
        i18n_file,
        PythonModuleName::parse(I18N).unwrap(),
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

fn realistic_specs() -> &'static RealisticSpecs {
    static SPECS: OnceLock<RealisticSpecs> = OnceLock::new();
    SPECS.get_or_init(build_realistic_specs)
}

/// Create a benchmark `Db` configured for semantic structure projections.
#[must_use]
pub fn structure_db() -> Db {
    let specs = realistic_specs();
    Db::new().with_tag_specs(specs.tag_specs.clone())
}

/// Create a benchmark `Db` configured with realistic Django tag specs,
/// template libraries, and filter arity data extracted from real Django
/// source files.
#[must_use]
pub fn realistic_db() -> Db {
    let specs = realistic_specs();

    Db::new()
        .with_tag_specs(specs.tag_specs.clone())
        .with_filter_arity_specs(specs.filter_arity_specs.clone())
}

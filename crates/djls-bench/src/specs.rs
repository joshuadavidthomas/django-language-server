use std::sync::OnceLock;

use camino::Utf8Path;
use djls_conf::TagSpecDef;
use djls_project::FilterArity;
use djls_project::FilterArityMap;
use djls_project::Interpreter;
use djls_project::Project;
use djls_project::PythonModuleName;
use djls_project::SearchPaths;
use djls_project::SymbolKey;
use djls_semantic::FilterAritySpecs;
use djls_semantic::TagSpecs;
use djls_source::Db as SourceDb;
use djls_testing::extract_bundle;

use crate::Db;

const DEFAULTTAGS: &str = "django.template.defaulttags";
const DEFAULTFILTERS: &str = "django.template.defaultfilters";
const I18N: &str = "django.templatetags.i18n";
struct RealisticSpecs {
    tag_specs: TagSpecs,
    filter_arity_specs: FilterAritySpecs,
    defaulttags_source: String,
    i18n_source: String,
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
        defaulttags_source,
        i18n_source,
    }
}

fn realistic_specs() -> &'static RealisticSpecs {
    static SPECS: OnceLock<RealisticSpecs> = OnceLock::new();
    SPECS.get_or_init(build_realistic_specs)
}

fn install_template_environment(db: &mut Db, specs: &RealisticSpecs) {
    // Custom builtin module identities preserve this benchmark's extracted-only workload. Using
    // Django's canonical module names would merge semantic's hardcoded builtin roles and change
    // the pinned diagnostic contract from source extraction to fallback-spec behavior.
    const SETTINGS: &str = "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'i18n': 'django.templatetags.i18n', 'static': 'django.templatetags.static'}, 'builtins': ['bench.defaulttags', 'bench.defaultfilters', 'bench.loader_tags']}}]\n";
    const DEFAULTFILTERS: &str = concat!(
        "from django import template\nregister = template.Library()\n",
        "@register.filter\ndef title(value): pass\n",
        "@register.filter\ndef lower(value): pass\n",
        "@register.filter\ndef upper(value): pass\n",
        "@register.filter\ndef default(value, arg): pass\n",
        "@register.filter\ndef date(value, arg=None): pass\n",
        "@register.filter\ndef truncatewords(value, arg): pass\n",
        "@register.filter\ndef floatformat(value, arg=None): pass\n",
        "@register.filter\ndef join(value, arg): pass\n",
        "@register.filter\ndef cut(value, arg): pass\n",
        "@register.filter\ndef yesno(value, arg=None): pass\n",
        "@register.filter\ndef pluralize(value, arg=None): pass\n",
        "@register.filter\ndef center(value, arg): pass\n",
    );
    const LOADER_TAGS: &str = concat!(
        "from django import template\nregister = template.Library()\n",
        "@register.tag\ndef block(parser, token): pass\n",
        "@register.tag\ndef extends(parser, token): pass\n",
        "@register.tag\ndef include(parser, token): pass\n",
    );

    for (path, source) in [
        ("/project/__init__.py", ""),
        ("/project/settings.py", SETTINGS),
        ("/django/__init__.py", ""),
        ("/django/template/__init__.py", ""),
        ("/django/template/defaultfilters.py", DEFAULTFILTERS),
        ("/django/template/loader_tags.py", LOADER_TAGS),
        ("/django/templatetags/__init__.py", ""),
        ("/bench/__init__.py", ""),
        ("/bench/defaultfilters.py", DEFAULTFILTERS),
        ("/bench/loader_tags.py", LOADER_TAGS),
        (
            "/django/templatetags/static.py",
            "from django import template\nregister = template.Library()\n@register.tag\ndef static(parser, token): pass\n",
        ),
    ] {
        db.add_fixture_source(path, source);
    }
    db.add_fixture_source(
        "/django/template/defaulttags.py",
        specs.defaulttags_source.clone(),
    );
    db.add_fixture_source("/bench/defaulttags.py", specs.defaulttags_source.clone());
    db.add_fixture_source("/django/templatetags/i18n.py", specs.i18n_source.clone());

    let root = Utf8Path::new("/");
    let interpreter = Interpreter::Auto;
    let search_paths =
        SearchPaths::from_project_settings(db.file_system(), root, &interpreter, &[]);
    search_paths.register_roots(db);
    let project = Project::new(
        db,
        root.to_path_buf(),
        search_paths,
        interpreter,
        Some(PythonModuleName::parse("project.settings").unwrap()),
        Vec::new(),
        Vec::new(),
        TagSpecDef::default(),
    );
    db.set_project(project);
}

/// Create a benchmark `Db` configured for semantic structure projections.
#[must_use]
pub fn structure_db() -> Db {
    let specs = realistic_specs();
    Db::new().with_projectless_tag_specs(specs.tag_specs.clone())
}

/// Create a benchmark `Db` configured with realistic Django tag specs,
/// template libraries, and filter arity data extracted from real Django
/// source files.
#[must_use]
pub fn realistic_db() -> Db {
    let specs = realistic_specs();
    let mut db = Db::new()
        .with_projectless_tag_specs(specs.tag_specs.clone())
        .with_projectless_filter_arity_specs(specs.filter_arity_specs.clone());
    install_template_environment(&mut db, specs);
    db
}

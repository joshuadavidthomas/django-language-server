use std::io;
use std::sync::Arc;
use std::sync::OnceLock;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::TagSpecDef;
use djls_ide::prime_template_library_products;
use djls_project::FilterArity;
use djls_project::FilterArityMap;
use djls_project::Interpreter;
use djls_project::InvalidModuleName;
use djls_project::Project;
use djls_project::PythonModuleName;
use djls_project::SearchPaths;
use djls_project::SymbolKey;
use djls_semantic::FilterAritySpecs;
use djls_semantic::TagSpecs;
use djls_source::Db as SourceDb;
use djls_source::FileError;
use djls_testing::extract_bundle;

use crate::Db;

const DEFAULTTAGS: &str = "django.template.defaulttags";
const DEFAULTFILTERS: &str = "django.template.defaultfilters";
const I18N: &str = "django.templatetags.i18n";

#[derive(Clone, Debug, thiserror::Error)]
pub enum BenchmarkSetupError {
    #[error("failed to read benchmark fixture {path}: {source}")]
    ReadFixture {
        path: Utf8PathBuf,
        #[source]
        source: Arc<io::Error>,
    },
    #[error("invalid benchmark Python module name {value:?}: {source}")]
    InvalidModuleName {
        value: &'static str,
        #[source]
        source: InvalidModuleName,
    },
    #[error("failed to register a benchmark source: {0}")]
    RegisterSource(#[from] FileError),
    #[error("the realistic benchmark fixture did not install a Project")]
    MissingProject,
}

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

fn build_realistic_specs() -> Result<RealisticSpecs, BenchmarkSetupError> {
    let mut tag_specs = TagSpecs::default();

    let fixture_root = crate::fixtures::crate_root().join("fixtures/python");
    let defaulttags_path = fixture_root.join("large/defaulttags.py");
    let defaulttags_source = std::fs::read_to_string(&defaulttags_path).map_err(|source| {
        BenchmarkSetupError::ReadFixture {
            path: defaulttags_path.clone(),
            source: Arc::new(source),
        }
    })?;
    let i18n_path = fixture_root.join("medium/i18n.py");
    let i18n_source =
        std::fs::read_to_string(&i18n_path).map_err(|source| BenchmarkSetupError::ReadFixture {
            path: i18n_path.clone(),
            source: Arc::new(source),
        })?;

    let mut extraction_db = Db::new();
    let defaulttags_file =
        extraction_db.file_with_contents(defaulttags_path, &defaulttags_source)?;
    let i18n_file = extraction_db.file_with_contents(i18n_path, &i18n_source)?;
    let defaulttags_module = PythonModuleName::parse(DEFAULTTAGS).map_err(|source| {
        BenchmarkSetupError::InvalidModuleName {
            value: DEFAULTTAGS,
            source,
        }
    })?;
    let i18n_module =
        PythonModuleName::parse(I18N).map_err(|source| BenchmarkSetupError::InvalidModuleName {
            value: I18N,
            source,
        })?;

    let defaulttags = extract_bundle(&extraction_db, defaulttags_file, defaulttags_module);
    tag_specs
        .merge_block_specs(&defaulttags.block_specs)
        .merge_tag_rules(&defaulttags.tag_rules);

    let i18n = extract_bundle(&extraction_db, i18n_file, i18n_module);
    tag_specs
        .merge_block_specs(&i18n.block_specs)
        .merge_tag_rules(&i18n.tag_rules);

    let filter_arity_specs = build_filter_arities(
        DEFAULTFILTERS,
        &[&defaulttags.filter_arities, &i18n.filter_arities],
    );

    Ok(RealisticSpecs {
        tag_specs,
        filter_arity_specs,
        defaulttags_source,
        i18n_source,
    })
}

fn realistic_specs() -> Result<&'static RealisticSpecs, BenchmarkSetupError> {
    static SPECS: OnceLock<Result<RealisticSpecs, BenchmarkSetupError>> = OnceLock::new();
    match SPECS.get_or_init(build_realistic_specs) {
        Ok(specs) => Ok(specs),
        Err(error) => Err(error.clone()),
    }
}

fn install_template_library_fixture(
    db: &mut Db,
    specs: &RealisticSpecs,
) -> Result<(), BenchmarkSetupError> {
    // Canonical builtin identities make extracted source facts fuse with semantic's hardcoded
    // Django roles and fallback grammar, matching production project analysis.
    const SETTINGS: &str = "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'i18n': 'django.templatetags.i18n', 'static': 'django.templatetags.static'}}}]\n";
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
    db.add_fixture_source("/django/templatetags/i18n.py", specs.i18n_source.clone());

    let root = Utf8Path::new("/");
    let interpreter = Interpreter::Auto;
    let search_paths =
        SearchPaths::from_project_settings(db.file_system(), root, &interpreter, &[]);
    search_paths.register_roots(db);
    let settings_module = PythonModuleName::parse("project.settings").map_err(|source| {
        BenchmarkSetupError::InvalidModuleName {
            value: "project.settings",
            source,
        }
    })?;
    let project = Project::new(
        db,
        root.to_path_buf(),
        search_paths,
        interpreter,
        Some(settings_module),
        Vec::new(),
        Vec::new(),
        TagSpecDef::default(),
    );
    db.set_project(project);
    Ok(())
}

/// Create a benchmark `Db` configured for semantic structure projections.
pub fn structure_db() -> Result<Db, BenchmarkSetupError> {
    let specs = realistic_specs()?;
    Ok(Db::new().with_projectless_tag_specs(specs.tag_specs.clone()))
}

/// Create a benchmark `Db` configured with realistic Django tag specs,
/// template libraries, and filter arity data extracted from real Django
/// source files.
pub fn realistic_db() -> Result<Db, BenchmarkSetupError> {
    configure_realistic_db(Db::new())
}

fn configure_realistic_db(db: Db) -> Result<Db, BenchmarkSetupError> {
    let specs = realistic_specs()?;
    let mut db = db
        .with_projectless_tag_specs(specs.tag_specs.clone())
        .with_projectless_filter_arity_specs(specs.filter_arity_specs.clone());
    install_template_library_fixture(&mut db, specs)?;
    Ok(db)
}

/// Create the realistic database with production intrinsic priming complete.
pub fn primed_realistic_db() -> Result<Db, BenchmarkSetupError> {
    let db = realistic_db()?;
    prime_template_library_products(&db).ok_or(BenchmarkSetupError::MissingProject)?;
    Ok(db)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use djls_project::Db as ProjectDb;
    use djls_project::ScopedTemplateLibraries;
    use djls_project::TemplateLibrary;
    use djls_project::template_library_catalog;
    use djls_semantic::SemanticOffsetContext;
    use djls_semantic::TagRole;
    use djls_semantic::tag_spec_at;
    use djls_source::Offset;
    use djls_templates::parse_template;
    use salsa::Event;

    use super::Db;
    use super::configure_realistic_db;
    use super::primed_realistic_db;

    impl Db {
        pub(crate) fn realistic_with_event_log(events: Arc<Mutex<Vec<Event>>>) -> Self {
            configure_realistic_db(Self::with_event_log(events))
                .expect("realistic benchmark database should initialize")
        }
    }

    #[test]
    fn realistic_project_fuses_canonical_builtins_and_converges_loads() {
        let mut db =
            primed_realistic_db().expect("primed realistic benchmark database should initialize");
        let project = db
            .project()
            .expect("realistic fixture should install a Project");
        let environment =
            ScopedTemplateLibraries::from_project_inventory(template_library_catalog(&db, project));
        let builtin_modules: Vec<_> = environment
            .resolved_libraries()
            .into_iter()
            .filter(|library| library.load_name().is_none())
            .map(TemplateLibrary::module_name_str)
            .collect();
        assert_eq!(
            builtin_modules,
            [
                "django.template.defaulttags",
                "django.template.defaultfilters",
                "django.template.loader_tags",
            ]
        );

        let source = "{% load i18n %}{% trans \"hello\" %}";
        let file = db
            .file_with_contents("/templates/semantic-contract.html", source)
            .expect("semantic contract fixture should register");
        let nodelist = parse_template(&db, file)
            .expect("semantic contract fixture should be a readable Template");
        let load_position = source
            .find("load")
            .expect("semantic contract source should contain the load tag");
        let load_position = u32::try_from(load_position)
            .expect("semantic contract load position should fit in u32");
        assert_eq!(
            tag_spec_at(&db, file, nodelist, load_position, "load").and_then(|spec| spec.role()),
            Some(TagRole::TemplateLibraryLoader),
        );

        let trans_position = source
            .find("trans")
            .expect("semantic contract source should contain the trans tag");
        let trans_position = u32::try_from(trans_position)
            .expect("semantic contract trans position should fit in u32");
        assert!(matches!(
            SemanticOffsetContext::from_offset(&db, file, Offset::new(trans_position)),
            SemanticOffsetContext::Tag {
                name,
                loaded_libraries,
                ..
            } if name == "trans" && loaded_libraries == ["i18n"]
        ));
    }
}

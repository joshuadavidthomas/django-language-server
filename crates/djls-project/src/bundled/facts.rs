//! Tag facts extracted once per process from an immutable bundled Django release.

use std::sync::Arc;
use std::sync::OnceLock;

use djls_conf::DjangoVersion;
use djls_conf::TagSpecDef;
use djls_source::FileSystem;
use djls_source::InMemoryFileSystem;
use djls_source::SourceFiles;

use super::BundledFileSystem;
use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonEnvironment;
use crate::python::PythonModuleName;
use crate::python::PythonSourceModule;
use crate::python::SearchPath;
use crate::python::SearchPaths;
use crate::python::python_module_name;
use crate::templates::BlockSpecs;
use crate::templates::TagRuleMap;
use crate::templates::TemplateLibraryId;
use crate::templates::TemplateSymbolKind;
use crate::templates::template_library_definition_facts;
use crate::templates::template_library_tag_facts;

/// Tag facts for one of Django's default builtin or standard loadable Template Libraries.
#[derive(Clone, Debug, PartialEq)]
pub struct BundledDjangoLibrary {
    module: PythonModuleName,
    tag_names: Vec<String>,
    tag_rules: TagRuleMap,
    block_specs: BlockSpecs,
}

impl BundledDjangoLibrary {
    #[must_use]
    pub fn module(&self) -> &PythonModuleName {
        &self.module
    }

    /// Registered Tag names, sorted.
    #[must_use]
    pub fn tag_names(&self) -> &[String] {
        &self.tag_names
    }

    #[must_use]
    pub fn tag_rules(&self) -> &TagRuleMap {
        &self.tag_rules
    }

    #[must_use]
    pub fn block_specs(&self) -> &BlockSpecs {
        &self.block_specs
    }
}

/// Extract Tag facts from Django's tag libraries in one bundled release.
///
/// The bundle is mounted as site-packages, exactly as Django Discovery mounts it for a project
/// without installed Django, so canonical-identity summaries such as `token_kwargs` apply. The
/// result is computed on first use and shared by every database in the process.
#[must_use]
pub fn bundled_django_libraries(version: DjangoVersion) -> &'static [BundledDjangoLibrary] {
    static LIBRARIES: [OnceLock<Vec<BundledDjangoLibrary>>; 3] =
        [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let index = match version {
        DjangoVersion::Django52 => 0,
        DjangoVersion::Django60 => 1,
        DjangoVersion::Django61 => 2,
    };
    LIBRARIES[index].get_or_init(|| {
        // Callers may be inside another database's query, and Salsa permits one attached
        // database per thread.
        std::thread::spawn(move || extract_libraries(version))
            .join()
            .unwrap_or_else(|_| {
                tracing::error!("Bundled Django tag fact extraction panicked");
                Vec::new()
            })
    })
}

fn extract_libraries(version: DjangoVersion) -> Vec<BundledDjangoLibrary> {
    let root = match super::source_root(version) {
        Ok(root) => root,
        Err(error) => {
            tracing::warn!(
                error_kind = ?error.kind(),
                "Could not prepare bundled Django sources for tag facts"
            );
            return Vec::new();
        }
    };
    let mut db = ExtractionDb {
        storage: salsa::Storage::default(),
        fs: Arc::new(BundledFileSystem::new(Arc::new(InMemoryFileSystem::new()))),
        files: SourceFiles::default(),
        project: None,
    };
    let search_paths = SearchPaths::from_paths(vec![SearchPath::SitePackages(root.clone())]);
    search_paths.register_roots(&db);
    db.project = Some(Project::new(
        &db,
        root,
        search_paths,
        PythonEnvironment::Auto,
        None,
        Some(version),
        Vec::new(),
        Vec::new(),
        TagSpecDef::default(),
    ));

    [
        python_module_name!(django.template.defaulttags),
        python_module_name!(django.template.defaultfilters),
        python_module_name!(django.template.loader_tags),
        python_module_name!(django.templatetags.cache),
        python_module_name!(django.templatetags.i18n),
        python_module_name!(django.templatetags.l10n),
        python_module_name!(django.templatetags.static),
        python_module_name!(django.templatetags.tz),
    ]
    .into_iter()
    .filter_map(|module| db.extract_library(module))
    .collect()
}

#[salsa::db]
#[derive(Clone)]
struct ExtractionDb {
    storage: salsa::Storage<Self>,
    fs: Arc<dyn FileSystem>,
    files: SourceFiles,
    project: Option<Project>,
}

impl ExtractionDb {
    fn extract_library(&self, module: PythonModuleName) -> Option<BundledDjangoLibrary> {
        let source = PythonSourceModule::resolve(self, self.project?, module.clone())?;
        let key = TemplateLibraryId::new(self, Some(source.file()), module.clone());
        let mut tag_names = template_library_definition_facts(self, key)
            .symbols()
            .filter(|symbol| symbol.kind == TemplateSymbolKind::Tag)
            .map(|symbol| symbol.name().to_string())
            .collect::<Vec<_>>();
        tag_names.sort();
        let facts = template_library_tag_facts(self, key);
        Some(BundledDjangoLibrary {
            module,
            tag_names,
            tag_rules: facts.tag_rules().clone(),
            block_specs: facts.block_specs().clone(),
        })
    }
}

#[salsa::db]
impl salsa::Database for ExtractionDb {}

#[salsa::db]
impl djls_source::Db for ExtractionDb {
    fn files(&self) -> &SourceFiles {
        &self.files
    }

    fn file_system(&self) -> &dyn FileSystem {
        self.fs.as_ref()
    }
}

#[salsa::db]
impl ProjectDb for ExtractionDb {
    fn project(&self) -> Option<Project> {
        self.project
    }
}

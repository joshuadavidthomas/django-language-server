use std::io;
use std::sync::Arc;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_project::Db as ProjectDb;
use djls_project::Project;
use djls_semantic::Db as SemanticDb;
use djls_semantic::FilterAritySpecs;
use djls_semantic::TagSpecs;
use djls_source::CaseSensitivity;
use djls_source::ChangeEvent;
use djls_source::Db as SourceDb;
use djls_source::File;
use djls_source::FileSystem;
use djls_source::FxDashMap;
use djls_source::RootWalk;
use djls_source::SourceChanges;
use djls_source::SourceFiles;
use djls_source::WalkEntry;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;
use djls_source::path_to_file;

#[derive(Clone)]
struct SourceMapFileSystem {
    sources: Arc<FxDashMap<Utf8PathBuf, String>>,
}

impl FileSystem for SourceMapFileSystem {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        Ok(self
            .sources
            .get(path)
            .map(|entry| entry.value().clone())
            .unwrap_or_default())
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        self.is_file(path) || self.is_dir(path)
    }

    fn is_file(&self, path: &Utf8Path) -> bool {
        self.sources.contains_key(path)
    }

    fn is_dir(&self, path: &Utf8Path) -> bool {
        self.sources
            .iter()
            .any(|entry| entry.key().starts_with(path))
            && !self.is_file(path)
    }

    fn case_sensitivity(&self) -> CaseSensitivity {
        CaseSensitivity::CaseSensitive
    }

    fn path_exists_case_sensitive(&self, path: &Utf8Path, _prefix: &Utf8Path) -> bool {
        self.exists(path)
    }

    fn walk_root(&self, root: &Utf8Path, options: &WalkOptions) -> RootWalk {
        if self.is_file(root) {
            return RootWalk::File(WalkEntry::file_root(root));
        }
        if !self.is_dir(root) {
            return RootWalk::Missing;
        }

        let mut entries = Vec::new();
        for path in self.sources.iter().map(|entry| entry.key().clone()) {
            if !path.starts_with(root) || path == root {
                continue;
            }

            let Ok(file_relative) = path.strip_prefix(root) else {
                continue;
            };
            let mut entry_path = root.to_path_buf();
            let mut entry_relative = Utf8PathBuf::new();
            for component in file_relative.components() {
                entry_path.push(component.as_str());
                entry_relative.push(component.as_str());

                if !options.hidden
                    && entry_relative.components().any(|component| {
                        component.as_str().starts_with('.') && component.as_str() != "."
                    })
                {
                    continue;
                }
                if let Some(max_depth) = options.max_depth
                    && entry_relative.components().count() > max_depth
                {
                    continue;
                }
                if entries
                    .iter()
                    .any(|entry: &WalkEntry| entry.path == entry_path)
                {
                    continue;
                }

                let kind = if self.is_file(&entry_path) {
                    WalkEntryKind::File
                } else if self.is_dir(&entry_path) {
                    WalkEntryKind::Directory
                } else {
                    WalkEntryKind::Other
                };
                entries.push(WalkEntry {
                    root: root.to_path_buf(),
                    path: entry_path.clone(),
                    relative: entry_relative.clone(),
                    kind,
                });
            }
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        entries.dedup_by(|left, right| left.path == right.path);
        RootWalk::Directory {
            entries,
            issues: Vec::new(),
        }
    }
}

#[salsa::db]
#[derive(Clone)]
pub struct Db {
    fs: SourceMapFileSystem,
    files: SourceFiles,
    tag_specs: Arc<TagSpecs>,
    filter_arity_specs: Arc<FilterAritySpecs>,
    storage: salsa::Storage<Self>,
}

impl Db {
    #[must_use]
    pub fn new() -> Self {
        Self {
            fs: SourceMapFileSystem {
                sources: Arc::new(FxDashMap::default()),
            },
            files: SourceFiles::default(),
            tag_specs: Arc::new(TagSpecs::default()),
            filter_arity_specs: Arc::new(FilterAritySpecs::new()),
            storage: salsa::Storage::default(),
        }
    }

    #[must_use]
    pub(crate) fn with_tag_specs(mut self, specs: TagSpecs) -> Self {
        self.tag_specs = Arc::new(specs);
        self
    }

    #[must_use]
    pub(crate) fn with_filter_arity_specs(mut self, specs: FilterAritySpecs) -> Self {
        self.filter_arity_specs = Arc::new(specs);
        self
    }

    /// Add source content and return the corresponding tracked file.
    ///
    /// # Panics
    ///
    /// Panics if the inserted benchmark source is not visible through the filesystem.
    pub fn file_with_contents(&mut self, path: impl Into<Utf8PathBuf>, contents: &str) -> File {
        let path = path.into();
        self.fs.sources.insert(path.clone(), contents.to_string());
        path_to_file(self, &path).expect("inserted benchmark source should be visible")
    }

    pub fn set_file_contents(&mut self, file: File, contents: &str) {
        let path = file.path(self).clone();
        self.fs.sources.insert(path.clone(), contents.to_string());
        SourceChanges::new([ChangeEvent::ContentChanged(path)]).apply(self);
    }
}

impl Default for Db {
    fn default() -> Self {
        Self::new()
    }
}

#[salsa::db]
impl salsa::Database for Db {}

#[salsa::db]
impl SourceDb for Db {
    fn files(&self) -> &SourceFiles {
        &self.files
    }

    fn file_system(&self) -> &dyn FileSystem {
        &self.fs
    }
}

#[salsa::db]
impl ProjectDb for Db {
    fn project(&self) -> Option<Project> {
        None
    }
}

#[salsa::db]
impl SemanticDb for Db {
    fn tag_specs(&self) -> &TagSpecs {
        &self.tag_specs
    }

    fn diagnostics_config(&self) -> djls_conf::DiagnosticsConfig {
        djls_conf::DiagnosticsConfig::default()
    }

    fn filter_arity_specs(&self) -> &FilterAritySpecs {
        &self.filter_arity_specs
    }

    fn model_graph(&self) -> &djls_project::ModelGraph {
        djls_project::ModelGraph::empty_ref()
    }
}

use std::collections::BTreeMap;
use std::collections::BTreeSet;
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
use djls_source::FileError;
use djls_source::FileSystem;
use djls_source::FxDashMap;
use djls_source::RootWalk;
use djls_source::SourceChanges;
use djls_source::SourceFiles;
use djls_source::WalkEntry;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;
use djls_source::path_to_file;
use salsa::Storage;

#[derive(Clone)]
struct SourceMapFileSystem {
    sources: Arc<FxDashMap<Utf8PathBuf, String>>,
    directories: Arc<FxDashMap<Utf8PathBuf, ()>>,
}

impl FileSystem for SourceMapFileSystem {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        let Some(source) = self.sources.get(path) else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("benchmark source {path} is not registered"),
            ));
        };
        Ok(source.value().clone())
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        self.is_file(path) || self.is_dir(path)
    }

    fn is_file(&self, path: &Utf8Path) -> bool {
        self.sources.contains_key(path)
    }

    fn is_dir(&self, path: &Utf8Path) -> bool {
        self.directories.contains_key(path) && !self.is_file(path)
    }

    fn case_sensitivity(&self) -> CaseSensitivity {
        CaseSensitivity::CaseSensitive
    }

    fn path_exists_case_sensitive(&self, path: &Utf8Path, _prefix: &Utf8Path) -> bool {
        self.exists(path)
    }

    fn walk_root(&self, root: &Utf8Path, options: &WalkOptions) -> RootWalk {
        let source_paths: BTreeSet<_> = self
            .sources
            .iter()
            .map(|entry| entry.key().clone())
            .collect();

        if source_paths.contains(root) {
            return RootWalk::File(WalkEntry::file_root(root));
        }
        if !source_paths.iter().any(|path| path.starts_with(root)) {
            return RootWalk::Missing;
        }

        let mut entries = BTreeMap::new();
        for path in &source_paths {
            if !path.starts_with(root) || path == root {
                continue;
            }

            let Ok(file_relative) = path.strip_prefix(root) else {
                continue;
            };
            let mut entry_path = root.to_path_buf();
            let mut entry_relative = Utf8PathBuf::new();
            for (depth, component) in file_relative.components().enumerate() {
                let component = component.as_str();
                entry_path.push(component);
                entry_relative.push(component);

                if !options.hidden && component.starts_with('.') && component != "." {
                    break;
                }
                if options
                    .max_depth
                    .is_some_and(|max_depth| depth + 1 > max_depth)
                {
                    break;
                }

                let kind = if source_paths.contains(&entry_path) {
                    WalkEntryKind::File
                } else {
                    WalkEntryKind::Directory
                };
                entries.entry(entry_path.clone()).or_insert(WalkEntry {
                    root: root.to_path_buf(),
                    path: entry_path.clone(),
                    relative: entry_relative.clone(),
                    kind,
                });
            }
        }
        RootWalk::Directory {
            entries: entries.into_values().collect(),
            issues: Vec::new(),
        }
    }
}

#[salsa::db]
#[derive(Clone)]
pub struct Db {
    fs: SourceMapFileSystem,
    files: SourceFiles,
    projectless_tag_specs: Arc<TagSpecs>,
    projectless_filter_arity_specs: Arc<FilterAritySpecs>,
    project: Option<Project>,
    storage: Storage<Self>,
}

impl Db {
    #[must_use]
    pub fn new() -> Self {
        Self::with_storage(Storage::default())
    }

    fn with_storage(storage: Storage<Self>) -> Self {
        Self {
            fs: SourceMapFileSystem {
                sources: Arc::new(FxDashMap::default()),
                directories: Arc::new(FxDashMap::default()),
            },
            files: SourceFiles::default(),
            projectless_tag_specs: Arc::new(TagSpecs::default()),
            projectless_filter_arity_specs: Arc::new(FilterAritySpecs::new()),
            project: None,
            storage,
        }
    }

    #[must_use]
    pub(crate) fn with_projectless_tag_specs(mut self, specs: TagSpecs) -> Self {
        self.projectless_tag_specs = Arc::new(specs);
        self
    }

    #[must_use]
    pub(crate) fn with_projectless_filter_arity_specs(mut self, specs: FilterAritySpecs) -> Self {
        self.projectless_filter_arity_specs = Arc::new(specs);
        self
    }

    pub(crate) fn add_fixture_source(
        &self,
        path: impl Into<Utf8PathBuf>,
        contents: impl Into<String>,
    ) {
        let path = path.into();
        for ancestor in path.ancestors().skip(1) {
            self.fs.directories.insert(ancestor.to_path_buf(), ());
        }
        self.fs.sources.insert(path, contents.into());
    }

    pub(crate) fn set_project(&mut self, project: Project) {
        self.project = Some(project);
    }

    /// Add source content and return the corresponding tracked file.
    pub fn file_with_contents(
        &mut self,
        path: impl Into<Utf8PathBuf>,
        contents: &str,
    ) -> Result<File, FileError> {
        let path = path.into();
        self.add_fixture_source(path.clone(), contents);
        path_to_file(self, &path)
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
        self.project
    }
}

#[salsa::db]
impl SemanticDb for Db {
    fn projectless_tag_specs(&self) -> &TagSpecs {
        &self.projectless_tag_specs
    }

    fn diagnostics_config(&self) -> djls_conf::DiagnosticsConfig {
        djls_conf::DiagnosticsConfig::default()
    }

    fn projectless_filter_arity_specs(&self) -> &FilterAritySpecs {
        &self.projectless_filter_arity_specs
    }

    fn model_graph(&self) -> &djls_project::ModelGraph {
        djls_project::ModelGraph::empty_ref()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_source::Db as SourceDb;
    use djls_source::RootWalk;
    use djls_source::WalkOptions;
    use salsa::Event;
    use salsa::Storage;

    use super::Db;

    impl Db {
        pub(crate) fn with_event_log(events: Arc<Mutex<Vec<Event>>>) -> Self {
            Self::with_storage(Storage::new(Some(Box::new(move |event| {
                events
                    .lock()
                    .expect("benchmark event log lock should not be poisoned")
                    .push(event);
            }))))
        }
    }

    fn walked_paths(db: &Db, options: &WalkOptions) -> Vec<Utf8PathBuf> {
        let directory = match db.file_system().walk_root(Utf8Path::new("/root"), options) {
            RootWalk::Directory { entries, issues } => Some((entries, issues)),
            RootWalk::File(_) | RootWalk::Missing | RootWalk::Inaccessible(_) => None,
        };
        let (entries, issues) = directory.expect("fixture root should be a directory");
        assert!(issues.is_empty());
        entries.into_iter().map(|entry| entry.path).collect()
    }

    #[test]
    fn source_map_read_reports_unregistered_paths() {
        let db = Db::new();
        let error = db
            .file_system()
            .read_to_string(Utf8Path::new("/missing.html"))
            .expect_err("an unregistered benchmark source should fail reads");

        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(
            error.to_string(),
            "benchmark source /missing.html is not registered"
        );
    }

    #[test]
    fn source_map_indexes_parent_directories() {
        let db = Db::new();
        db.add_fixture_source("/root/a/nested.py", "");

        assert!(db.file_system().is_dir(Utf8Path::new("/root")));
        assert!(db.file_system().is_dir(Utf8Path::new("/root/a")));
        assert!(!db.file_system().is_dir(Utf8Path::new("/root/a/nested.py")));
        assert!(!db.file_system().is_dir(Utf8Path::new("/root/missing")));
    }

    #[test]
    fn source_map_walk_is_sorted_deduplicated_and_respects_hidden_and_depth() {
        let db = Db::new();
        db.add_fixture_source("/root/b/second.py", "");
        db.add_fixture_source("/root/a/first.py", "");
        db.add_fixture_source("/root/a/nested/third.py", "");
        db.add_fixture_source("/root/.hidden/secret.py", "");

        assert_eq!(
            walked_paths(&db, &WalkOptions::project()),
            [
                Utf8PathBuf::from("/root/a"),
                Utf8PathBuf::from("/root/a/first.py"),
                Utf8PathBuf::from("/root/a/nested"),
                Utf8PathBuf::from("/root/a/nested/third.py"),
                Utf8PathBuf::from("/root/b"),
                Utf8PathBuf::from("/root/b/second.py"),
            ]
        );
        assert_eq!(
            walked_paths(&db, &WalkOptions::shallow()),
            [
                Utf8PathBuf::from("/root/.hidden"),
                Utf8PathBuf::from("/root/a"),
                Utf8PathBuf::from("/root/b"),
            ]
        );
    }
}

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::io;
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

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
    storage: salsa::Storage<Self>,
}

impl Db {
    #[must_use]
    pub fn new() -> Self {
        Self::with_storage(salsa::Storage::default())
    }

    #[cfg(test)]
    pub(crate) fn with_event_log(events: Arc<Mutex<Vec<salsa::Event>>>) -> Self {
        Self::with_storage(salsa::Storage::new(Some(Box::new(move |event| {
            events
                .lock()
                .expect("benchmark event log lock should not be poisoned")
                .push(event);
        }))))
    }

    fn with_storage(storage: salsa::Storage<Self>) -> Self {
        Self {
            fs: SourceMapFileSystem {
                sources: Arc::new(FxDashMap::default()),
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
        self.fs.sources.insert(path.into(), contents.into());
    }

    pub(crate) fn set_project(&mut self, project: Project) {
        self.project = Some(project);
    }

    /// Add source content and return the corresponding tracked file.
    ///
    /// # Panics
    ///
    /// Panics if the inserted benchmark source is not visible through the filesystem.
    pub fn file_with_contents(&mut self, path: impl Into<Utf8PathBuf>, contents: &str) -> File {
        let path = path.into();
        self.add_fixture_source(path.clone(), contents);
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
    use super::*;

    fn walked_paths(db: &Db, options: &WalkOptions) -> Vec<Utf8PathBuf> {
        let RootWalk::Directory { entries, issues } =
            db.file_system().walk_root(Utf8Path::new("/root"), options)
        else {
            panic!("fixture root should be a directory");
        };
        assert!(issues.is_empty());
        entries.into_iter().map(|entry| entry.path).collect()
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

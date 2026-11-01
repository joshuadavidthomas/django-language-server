use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;

use camino::Utf8Path;
use djls_conf::Settings;
use djls_db::DjangoDatabase;
use djls_ide::PrimedTemplateLibraries;
use djls_lsp::Workspace;
use djls_project::EnvironmentAssemblyError;
use djls_project::run_django_discovery;
use djls_source::ChangeEvent;
use djls_source::File;
use djls_source::FileKind;
use djls_source::FileSystem;
use djls_source::InMemoryFileSystem;
use djls_source::SourceChanges;
use djls_source::path_to_file;
use tower_lsp_server::ls_types;

/// An in-memory Django project: synced files, open editor buffers over them, and the
/// production database reading through both.
pub(crate) struct Project {
    /// The project as the host last synced it; open buffers take precedence.
    files: Arc<Mutex<InMemoryFileSystem>>,
    workspace: Workspace,
    db: DjangoDatabase,
    /// Sources covered by the last successful Django Discovery, or `None` before one.
    primed: Option<PrimedTemplateLibraries>,
}

impl Project {
    pub(crate) fn new(root: Option<&Utf8Path>, settings: Settings) -> Self {
        // Embedded releases must be extracted before a database exists; see `registerDjango`.
        djls_project::prime_bundled_django_libraries();
        let files = Arc::new(Mutex::new(InMemoryFileSystem::new()));
        let workspace = Workspace::with_disk(Arc::clone(&files) as Arc<dyn FileSystem>);
        let mut db = DjangoDatabase::new(workspace.file_system(), &settings, root);
        db.apply_project_settings(settings);
        Self {
            files,
            workspace,
            db,
            primed: None,
        }
    }

    pub(crate) fn db(&self) -> &DjangoDatabase {
        &self.db
    }

    pub(crate) fn db_mut(&mut self) -> &mut DjangoDatabase {
        &mut self.db
    }

    pub(crate) fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub(crate) fn workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspace
    }

    /// Add or replace a synced file and return the change it makes visible. An open
    /// buffer hides the synced text until it closes.
    pub(crate) fn write_file(&mut self, path: &Utf8Path, content: String) -> Option<ChangeEvent> {
        let existed = {
            let mut files = self.files.lock().unwrap_or_else(PoisonError::into_inner);
            let existed = files.is_file(path);
            files.add_file(path.to_path_buf(), content);
            existed
        };
        let change = if self.workspace.contains_document(path) {
            return None;
        } else if existed {
            ChangeEvent::ContentChanged(path.to_path_buf())
        } else {
            ChangeEvent::BecameVisible(path.to_path_buf())
        };
        self.apply(change.clone());
        Some(change)
    }

    pub(crate) fn remove_file(&mut self, path: &Utf8Path) -> Option<ChangeEvent> {
        let existed = {
            let mut files = self.files.lock().unwrap_or_else(PoisonError::into_inner);
            let existed = files.is_file(path);
            files.remove_file(path);
            existed
        };
        if !existed || self.workspace.contains_document(path) {
            return None;
        }
        let change = ChangeEvent::Deleted(path.to_path_buf());
        self.apply(change.clone());
        Some(change)
    }

    /// Apply a change after the filesystem view (synced files or buffers) already shows it.
    pub(crate) fn apply(&mut self, change: ChangeEvent) {
        SourceChanges::new([change]).apply(&mut self.db);
    }

    /// Whether `change` can alter what Django Discovery finds: membership changes, settings
    /// sources, and Python environment metadata. Before a discovery has succeeded, any
    /// Python source might be a settings dependency.
    pub(crate) fn needs_discovery(&self, change: &ChangeEvent) -> bool {
        let path = match change {
            ChangeEvent::BecameVisible(_) | ChangeEvent::Deleted(_) | ChangeEvent::Rescan => {
                return true;
            }
            ChangeEvent::Opened(path) | ChangeEvent::ContentChanged(path) => path,
        };
        if path.file_name() == Some("pyvenv.cfg") || path.extension() == Some("pth") {
            return true;
        }
        if FileKind::from(path) != FileKind::Python {
            return false;
        }
        let Some(primed) = &self.primed else {
            return true;
        };
        primed
            .full_reload_files()
            .iter()
            .any(|file| file.path(&self.db) == path)
    }

    /// Run Django Discovery, then prime Template Library facts as the server's readiness does.
    pub(crate) fn discover(&mut self) -> Result<(), EnvironmentAssemblyError> {
        self.primed = None;
        run_django_discovery(&mut self.db)?;
        self.primed = djls_ide::prime_template_library_products(&self.db);
        Ok(())
    }

    /// Whether `path` is a synced file or open document rather than, say, a bundled source.
    pub(crate) fn is_workspace_file(&self, path: &Utf8Path) -> bool {
        self.workspace.contains_document(path)
            || self
                .files
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_file(path)
    }

    pub(crate) fn file(&self, path: &Utf8Path) -> Option<File> {
        path_to_file(&self.db, path).ok()
    }

    /// The file at `path` if IDE features apply to it, as the server checks per request.
    pub(crate) fn template(&self, path: &Utf8Path) -> Option<File> {
        self.file(path).filter(|file| self.is_template(*file))
    }

    pub(crate) fn is_template(&self, file: File) -> bool {
        matches!(file.try_source(&self.db), Ok(source) if *source.kind() == FileKind::Template)
    }

    /// Like the server, this honors `format.enabled` (off by default) and `format.backend`.
    pub(crate) fn format(
        &self,
        file: File,
        encoding: djls_source::PositionEncoding,
        options: &ls_types::FormattingOptions,
    ) -> Vec<ls_types::TextEdit> {
        let format_config = self.db.settings().format().clone();
        if !format_config.enabled() || !self.is_template(file) {
            return Vec::new();
        }
        djls_ide::format_document(&self.db, file, encoding, format_config.backend(), options)
    }
}

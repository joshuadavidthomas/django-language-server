//! The synchronous session both LSP hosts wrap.
//!
//! [`SessionCore`] owns the database, the open-document [`Workspace`], and the negotiated
//! [`ClientInfo`]. It applies document and file mutations to the database and classifies the
//! Project work each one calls for; hosts decide when and how that work runs. [`Analysis`] is
//! the read side requests compute against, either borrowed from the core or cloned as a
//! snapshot.
//!
//! The API is path-based: hosts map their document URIs to database paths.

use std::sync::Arc;

use camino::Utf8Path;
use djls_db::DjangoDatabase;
use djls_ide::PrimedTemplateLibraries;
use djls_project::Db as _;
use djls_project::EnvironmentAssemblyError;
use djls_source::ChangeEvent;
use djls_source::Db as _;
use djls_source::File;
use djls_source::FileKind;
use djls_source::Offset;
use djls_source::SourceChanges;
use djls_source::Span;
use djls_source::path_to_file;
use tower_lsp_server::ls_types;

use crate::client::ClientInfo;
use crate::document::TextDocument;
use crate::ext::InitializeParamsExt;
use crate::ext::PositionExt;
use crate::ext::TextDocumentContentChangeEventExt;
use crate::ext::TextDocumentItemExt;
use crate::workspace::Workspace;

/// Work a source change requires before project-aware results are current again.
///
/// Ordered by cost, so merging pending work is `max`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum ProjectWork {
    /// Re-prime Template Library products; Django Discovery's results still hold.
    Reprime,
    /// Rerun Django Discovery, then prime.
    FullReload,
}

/// The sources the last successful priming depends on, which decide whether an edit needs
/// a re-prime or a full reload.
#[derive(Clone, Debug)]
pub struct ProjectCoverage {
    reprime_files: Arc<[File]>,
    full_reload_files: Arc<[File]>,
}

impl ProjectCoverage {
    #[must_use]
    pub fn new(
        reprime_files: impl Into<Arc<[File]>>,
        full_reload_files: impl Into<Arc<[File]>>,
    ) -> Self {
        Self {
            reprime_files: reprime_files.into(),
            full_reload_files: full_reload_files.into(),
        }
    }

    #[must_use]
    pub fn reprime_files(&self) -> &[File] {
        &self.reprime_files
    }

    /// This coverage with `file` added to the re-prime set.
    #[must_use]
    pub fn with_reprime_file(&self, file: File) -> Self {
        let reprime_files = if self.reprime_files.contains(&file) {
            Arc::clone(&self.reprime_files)
        } else {
            let mut files = self.reprime_files.to_vec();
            files.push(file);
            files.into()
        };
        Self {
            reprime_files,
            full_reload_files: Arc::clone(&self.full_reload_files),
        }
    }
}

impl From<&PrimedTemplateLibraries> for ProjectCoverage {
    fn from(primed: &PrimedTemplateLibraries) -> Self {
        Self::new(primed.reprime_files(), primed.full_reload_files())
    }
}

/// An applied document mutation.
pub struct DocumentUpdate {
    /// The buffer after the mutation; for a close, the buffer that closed.
    pub document: Arc<TextDocument>,
    pub project_work: Option<ProjectWork>,
}

pub struct SessionCore {
    workspace: Workspace,
    analysis: Analysis,
}

impl SessionCore {
    /// A session over `workspace` for the project at `root`, configured by the client's
    /// initialization options.
    #[must_use]
    pub fn new(
        workspace: Workspace,
        root: Option<&Utf8Path>,
        params: &ls_types::InitializeParams,
    ) -> Self {
        let options = params.client_options();
        let db = DjangoDatabase::new(workspace.file_system(), &options.settings, root);
        let client_info =
            ClientInfo::new(&params.capabilities, params.client_info.as_ref(), options);
        Self {
            workspace,
            analysis: Analysis { db, client_info },
        }
    }

    #[must_use]
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    #[must_use]
    pub fn analysis(&self) -> &Analysis {
        &self.analysis
    }

    /// An owned copy of the read side for computing off the session.
    #[must_use]
    pub fn snapshot(&self) -> Analysis {
        self.analysis.clone()
    }

    #[must_use]
    pub fn db(&self) -> &DjangoDatabase {
        &self.analysis.db
    }

    pub fn db_mut(&mut self) -> &mut DjangoDatabase {
        &mut self.analysis.db
    }

    #[must_use]
    pub fn client_info(&self) -> &ClientInfo {
        &self.analysis.client_info
    }

    /// Open `document` as the buffer for `path`.
    pub fn open_document(
        &mut self,
        path: &Utf8Path,
        document: &ls_types::TextDocumentItem,
        coverage: Option<&ProjectCoverage>,
    ) -> DocumentUpdate {
        let kind = document.language_id_to_file_kind(self.client_info().client());
        let change = self.workspace.open_document_change(self.db(), path);
        let document = self
            .workspace
            .open_document(path, &document.text, document.version, kind);
        self.document_update(&change, document, coverage)
    }

    /// Apply edits to the buffer for `path`. A full-text edit to an unopened path opens it.
    pub fn update_document(
        &mut self,
        path: &Utf8Path,
        version: i32,
        changes: Vec<ls_types::TextDocumentContentChangeEvent>,
        coverage: Option<&ProjectCoverage>,
    ) -> Option<DocumentUpdate> {
        let change = if self.workspace.contains_document(path) {
            ChangeEvent::ContentChanged(path.to_path_buf())
        } else {
            self.workspace.open_document_change(self.db(), path)
        };
        let document = self.workspace.update_document(
            path,
            changes.to_document_changes(),
            version,
            self.client_info().position_encoding(),
        )?;
        Some(self.document_update(&change, document, coverage))
    }

    /// Reapply the saved buffer for `path` as a content change.
    pub fn save_document(
        &mut self,
        path: &Utf8Path,
        coverage: Option<&ProjectCoverage>,
    ) -> Option<DocumentUpdate> {
        let document = self.workspace.save_document(path)?;
        Some(self.document_update(
            &ChangeEvent::ContentChanged(path.to_path_buf()),
            document,
            coverage,
        ))
    }

    /// Close the buffer for `path`, so reads fall back to the files beneath it.
    pub fn close_document(
        &mut self,
        path: &Utf8Path,
        coverage: Option<&ProjectCoverage>,
    ) -> Option<DocumentUpdate> {
        let change = self.workspace.close_document_change(path);
        let document = self.workspace.close_document(path)?;
        Some(self.document_update(&change, document, coverage))
    }

    /// Apply `change` to the files beneath the buffers, which already show it. An open buffer
    /// hides the change until it closes.
    pub fn file_changed(
        &mut self,
        change: &ChangeEvent,
        coverage: Option<&ProjectCoverage>,
    ) -> Option<ProjectWork> {
        let path = changed_path(change)?;
        if self.workspace.contains_document(path) {
            return None;
        }
        let kind = FileKind::from(path);
        SourceChanges::new([change.clone()]).apply(self.db_mut());
        self.project_work(change, kind, coverage)
    }

    fn document_update(
        &mut self,
        change: &ChangeEvent,
        document: Arc<TextDocument>,
        coverage: Option<&ProjectCoverage>,
    ) -> DocumentUpdate {
        SourceChanges::new([change.clone()]).apply(self.db_mut());
        let project_work = self.project_work(change, document.kind(), coverage);
        DocumentUpdate {
            document,
            project_work,
        }
    }

    /// The Project work an applied `change` to a `kind` file requires, given the coverage of
    /// the last successful priming (`None` before one succeeds).
    ///
    /// Documents report their client language kind rather than the path-derived one, so an
    /// editor's language choice decides whether a buffer participates in invalidation.
    #[must_use]
    pub fn project_work(
        &self,
        change: &ChangeEvent,
        kind: FileKind,
        coverage: Option<&ProjectCoverage>,
    ) -> Option<ProjectWork> {
        let db = self.db();
        db.project()?;
        let path = changed_path(change)?;
        if kind != FileKind::Python {
            return None;
        }

        let Some(coverage) = coverage else {
            // Until coverage publishes, every Python source is a possible settings or
            // catalog dependency. Full discovery is the only safe classification.
            return Some(ProjectWork::FullReload);
        };
        let changed_membership = matches!(
            change,
            ChangeEvent::BecameVisible(_) | ChangeEvent::Deleted(_)
        );
        if changed_membership
            || coverage
                .full_reload_files
                .iter()
                .any(|file| file.path(db) == path)
        {
            return Some(ProjectWork::FullReload);
        }
        // Readiness covers inventory and structure, not every lazy rule dependency, so any
        // tracked Python source under a registered root re-primes.
        let reprime = coverage
            .reprime_files
            .iter()
            .any(|file| file.path(db) == path)
            || (db.files().root(db, path).is_some() && db.files().try_file(path).is_some());
        reprime.then_some(ProjectWork::Reprime)
    }

    /// Run `work` on this thread and return the coverage it publishes, `None` without a
    /// Project. For hosts that cannot run it in the background.
    ///
    /// # Errors
    ///
    /// Returns an error when Django Discovery cannot assemble the Python Environment.
    pub fn run_project_work(
        &mut self,
        work: ProjectWork,
    ) -> Result<Option<ProjectCoverage>, EnvironmentAssemblyError> {
        if work == ProjectWork::FullReload {
            djls_project::run_django_discovery(self.db_mut())?;
        }
        Ok(djls_ide::prime_template_library_products(self.db())
            .map(|primed| ProjectCoverage::from(&primed)))
    }
}

fn changed_path(change: &ChangeEvent) -> Option<&Utf8Path> {
    match change {
        ChangeEvent::Opened(path)
        | ChangeEvent::BecameVisible(path)
        | ChangeEvent::ContentChanged(path)
        | ChangeEvent::Deleted(path) => Some(path),
        ChangeEvent::Rescan => None,
    }
}

/// What requests read: the database and the client's negotiated capabilities.
#[derive(Clone)]
pub struct Analysis {
    db: DjangoDatabase,
    client_info: ClientInfo,
}

impl Analysis {
    #[must_use]
    pub fn db(&self) -> &DjangoDatabase {
        &self.db
    }

    #[must_use]
    pub fn client_info(&self) -> &ClientInfo {
        &self.client_info
    }

    /// The tracked file at `path`.
    ///
    /// Open buffers reach the database through the workspace overlay, so features read
    /// current text through [`File::source`] rather than buffer state.
    #[must_use]
    pub fn file(&self, path: &Utf8Path) -> Option<File> {
        path_to_file(&self.db, path).ok()
    }

    /// The file at `path` if it is a template, the only kind IDE features apply to.
    #[must_use]
    pub fn template(&self, path: &Utf8Path) -> Option<File> {
        self.file(path).filter(|file| {
            matches!(file.try_source(&self.db), Ok(source) if *source.kind() == FileKind::Template)
        })
    }

    #[must_use]
    pub fn template_position(
        &self,
        path: &Utf8Path,
        position: ls_types::Position,
    ) -> Option<(File, Offset)> {
        let file = self.template(path)?;
        let source = file.try_source(&self.db).ok()?;
        let offset = position.to_offset(
            source.as_str(),
            file.line_index(&self.db),
            self.client_info.position_encoding(),
        );
        Some((file, offset))
    }

    #[must_use]
    pub fn template_range(&self, path: &Utf8Path, range: ls_types::Range) -> Option<(File, Span)> {
        let file = self.template(path)?;
        let source = file.try_source(&self.db).ok()?;
        let line_index = file.line_index(&self.db);
        let encoding = self.client_info.position_encoding();
        let start = range.start.to_offset(source.as_str(), line_index, encoding);
        let end = range.end.to_offset(source.as_str(), line_index, encoding);
        let span = Span::saturating_from_bounds_usize(start.get() as usize, end.get() as usize);
        Some((file, span))
    }

    /// Edits formatting the template at `path`, honoring `format.enabled` (off by default)
    /// and `format.backend`. Syntax-only: needs no Project readiness.
    #[must_use]
    pub fn format(
        &self,
        path: &Utf8Path,
        options: &ls_types::FormattingOptions,
    ) -> Vec<ls_types::TextEdit> {
        let format_config = self.db.settings().format().clone();
        if !format_config.enabled() {
            return Vec::new();
        }
        let Some(file) = self.template(path) else {
            return Vec::new();
        };
        djls_ide::format_document(
            &self.db,
            file,
            self.client_info.position_encoding(),
            format_config.backend(),
            options,
        )
    }
}

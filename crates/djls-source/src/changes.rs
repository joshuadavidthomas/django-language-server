use camino::Utf8Path;
use camino::Utf8PathBuf;

use crate::Db;
use crate::File;
use crate::files::sync_known_paths;
use crate::path_to_file;

/// Source-visible filesystem changes applied after the backing filesystem view
/// has been updated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceChanges {
    events: Vec<ChangeEvent>,
}

impl SourceChanges {
    #[must_use]
    pub fn new(events: impl IntoIterator<Item = ChangeEvent>) -> Self {
        Self {
            events: events.into_iter().collect(),
        }
    }

    /// Apply these source-visible file changes to Salsa inputs.
    ///
    /// Callers must update the underlying filesystem view first. For LSP
    /// documents, this means inserting or removing the editor buffer from the
    /// overlay before applying the corresponding event.
    pub fn apply(&self, db: &mut dyn Db) {
        for change in &self.events {
            match change {
                ChangeEvent::Opened(path) => {
                    apply_visible_path_change(db, path, FileSetMembership::Unchanged);
                }
                ChangeEvent::BecameVisible(path) => {
                    apply_visible_path_change(db, path, FileSetMembership::Changed);
                }
                ChangeEvent::ContentChanged(path) => apply_content_change(db, path),
                ChangeEvent::Deleted(path) => apply_deleted_path(db, path),
                ChangeEvent::Rescan => sync_known_paths(db),
            }
        }
    }
}

/// One source-visible filesystem change in a `SourceChanges` batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeEvent {
    /// An existing visible file was opened in an editor buffer.
    Opened(Utf8PathBuf),
    /// A path became visible to source queries.
    BecameVisible(Utf8PathBuf),
    /// A visible file's content changed without changing the file set.
    ContentChanged(Utf8PathBuf),
    /// A path stopped being visible to source queries.
    Deleted(Utf8PathBuf),
    /// Refresh every already-tracked path when precise path events are unavailable.
    ///
    /// This does not discover new paths. It re-checks statuses for paths that
    /// source queries have already interned, including missing paths recorded
    /// during resolution.
    Rescan,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum FileSetMembership {
    Changed,
    Unchanged,
}

fn apply_visible_path_change(
    db: &mut dyn Db,
    path: &Utf8Path,
    file_set_membership: FileSetMembership,
) {
    File::sync_path(db, path);
    if path_to_file(db, path).is_err() {
        return;
    }

    if matches!(file_set_membership, FileSetMembership::Changed)
        && let Some(root) = db.files().root(db, path)
    {
        db.bump_file_root_revision(root);
    }
}

fn apply_content_change(db: &mut dyn Db, path: &Utf8Path) {
    let Some(file) = db.files().try_file(path) else {
        return;
    };

    file.sync(db);
}

fn apply_deleted_path(db: &mut dyn Db, path: &Utf8Path) {
    File::sync_path(db, path);

    if let Some(root) = db.files().root(db, path) {
        db.bump_file_root_revision(root);
    }
}

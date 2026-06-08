use camino::Utf8Path;
use salsa::Setter;

use crate::File;
use crate::FileSystem;
use crate::SourceFiles;
use crate::WalkEntry;
use crate::WalkOptions;

#[salsa::db]
pub trait Db: salsa::Database {
    fn files(&self) -> &SourceFiles;

    fn file_system(&self) -> &dyn FileSystem;

    fn read_file(&self, path: &Utf8Path) -> std::io::Result<String> {
        self.file_system().read_to_string(path)
    }

    fn path_exists(&self, path: &Utf8Path) -> bool {
        self.file_system().exists(path)
    }

    fn path_is_file(&self, path: &Utf8Path) -> bool {
        self.file_system().is_file(path)
    }

    fn path_is_dir(&self, path: &Utf8Path) -> bool {
        self.file_system().is_dir(path)
    }

    fn walk_entries(
        &self,
        root: &Utf8Path,
        options: &WalkOptions,
    ) -> std::io::Result<Vec<WalkEntry>> {
        self.file_system().walk_entries(root, options)
    }

    /// Get or create a tracked file for the given path.
    fn get_or_create_file(&self, path: &Utf8Path) -> File {
        self.files().get_or_create_file(self, path)
    }

    /// Bump the revision for a tracked file to invalidate dependent queries.
    fn bump_file_revision(&mut self, file: File) {
        let current_rev = file.revision(self);
        let new_rev = current_rev + 1;
        file.set_revision(self).to(new_rev);
    }
}

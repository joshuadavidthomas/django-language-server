use camino::Utf8Path;
use salsa::Setter;

use crate::File;
use crate::SourceFiles;

#[salsa::db]
pub trait Db: salsa::Database {
    fn files(&self) -> &SourceFiles;

    fn read_file(&self, path: &Utf8Path) -> std::io::Result<String>;

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

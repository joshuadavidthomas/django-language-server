use camino::Utf8Path;
use salsa::Setter;

use crate::File;

#[salsa::db]
pub trait Db: salsa::Database {
    fn create_file(&self, path: &Utf8Path) -> File;

    /// Look up a tracked file if it exists.
    fn get_file(&self, path: &Utf8Path) -> Option<File>;

    fn read_file(&self, path: &Utf8Path) -> std::io::Result<String>;

    /// Get or create a tracked file for the given path.
    fn get_or_create_file(&self, path: &Utf8Path) -> File {
        if let Some(entry) = self.get_file(path) {
            return entry;
        }
        self.create_file(path)
    }

    /// Bump the revision for a tracked file to invalidate dependent queries.
    fn bump_file_revision(&mut self, file: File) {
        let current_rev = file.revision(self);
        let new_rev = current_rev + 1;
        file.set_revision(self).to(new_rev);
    }

    /// Get or create a tracked file for the given path and bump its revision.
    fn invalidate_file(&mut self, path: &Utf8Path) -> File {
        let file = self.get_or_create_file(path);
        self.bump_file_revision(file);
        file
    }
}

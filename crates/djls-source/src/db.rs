use camino::Utf8Path;
use salsa::Setter;

use crate::File;
use crate::FileRoot;
use crate::FileSystem;
use crate::RootWalk;
use crate::SourceFiles;
use crate::WalkOptions;

#[salsa::db]
pub trait Db: salsa::Database {
    fn files(&self) -> &SourceFiles;

    fn file_system(&self) -> &dyn FileSystem;

    fn read_file(&self, path: &Utf8Path) -> std::io::Result<String> {
        self.file_system().read_to_string(path)
    }

    fn path_is_file(&self, path: &Utf8Path) -> bool {
        self.file_system().is_file(path)
    }

    fn path_is_dir(&self, path: &Utf8Path) -> bool {
        self.file_system().is_dir(path)
    }

    fn walk_root(&self, root: &Utf8Path, options: &WalkOptions) -> RootWalk {
        self.file_system().walk_root(root, options)
    }

    /// Bump the revision for a tracked file to invalidate dependent queries.
    fn bump_file_revision(&mut self, file: File) {
        let current_rev = file.revision(self);
        let new_rev = current_rev + 1;
        file.set_revision(self).to(new_rev);
    }

    /// Bump the revision for a tracked source root to invalidate dependent queries.
    fn bump_file_root_revision(&mut self, root: FileRoot) {
        let current_rev = root.revision(self);
        let new_rev = current_rev + 1;
        root.set_revision(self).to(new_rev);
    }
}

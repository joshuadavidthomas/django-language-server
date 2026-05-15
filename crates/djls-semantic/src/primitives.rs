use camino::Utf8PathBuf;
use djls_source::File;

use crate::db::Db as SemanticDb;

#[salsa::tracked]
pub struct Template<'db> {
    pub name: TemplateName<'db>,
    pub file: File,
}

impl<'db> Template<'db> {
    pub fn path_buf(&'db self, db: &'db dyn SemanticDb) -> &'db Utf8PathBuf {
        self.file(db).path(db)
    }
}

#[salsa::interned]
pub struct TemplateName {
    #[returns(ref)]
    pub name: String,
}

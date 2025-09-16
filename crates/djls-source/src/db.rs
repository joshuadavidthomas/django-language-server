use camino::Utf8Path;

#[salsa::db]
pub trait Db: salsa::Database {
    fn read_file_source(&self, path: &Utf8Path) -> std::io::Result<String>;
}

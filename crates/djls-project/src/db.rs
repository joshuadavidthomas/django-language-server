use crate::ProjectLoadingState;

#[salsa::db]
pub trait Db: djls_source::Db {
    fn project_loading_state(&self) -> ProjectLoadingState;
}

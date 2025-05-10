use crate::meta::ProjectMetadata;

#[salsa::db]
pub trait Db: salsa::Database {
    fn metadata(&self) -> &ProjectMetadata;
}

#[salsa::db]
#[derive(Clone)]
pub struct ProjectDatabase {
    storage: salsa::Storage<ProjectDatabase>,
    metadata: ProjectMetadata,
}

impl ProjectDatabase {
    pub fn new(metadata: ProjectMetadata) -> Self {
        let storage = salsa::Storage::new(None);

        Self { storage, metadata }
    }
}

#[salsa::db]
impl Db for ProjectDatabase {
    fn metadata(&self) -> &ProjectMetadata {
        &self.metadata
    }
}

#[salsa::db]
impl salsa::Database for ProjectDatabase {}

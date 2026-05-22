use salsa::Setter;

use crate::ProjectLoadingState;
use crate::ProjectSourceFilesAvailability;

#[salsa::db]
pub trait Db: djls_source::Db {
    fn project_loading_state(&self) -> ProjectLoadingState;

    fn set_project_source_files_availability(
        &mut self,
        availability: ProjectSourceFilesAvailability,
    ) {
        let state = self.project_loading_state();
        state.set_source_files(self).to(availability);
    }
}

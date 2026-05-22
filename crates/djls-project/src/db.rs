use salsa::Setter;

use crate::ProjectLoadingState;
use crate::ProjectSourceFilesAvailability;

#[salsa::db]
pub trait Db: djls_source::Db {
    fn project_loading_state(&self) -> ProjectLoadingState;

    fn begin_project_loading_run(&mut self) {
        let previous = match self.project_loading_state().source_files(self) {
            ProjectSourceFilesAvailability::Ready(files)
            | ProjectSourceFilesAvailability::Stale { previous: files } => Some(files),
            ProjectSourceFilesAvailability::Deferred { previous, .. }
            | ProjectSourceFilesAvailability::Unavailable { previous, .. }
            | ProjectSourceFilesAvailability::Failed { previous, .. } => previous,
            ProjectSourceFilesAvailability::Loading => None,
        };
        let availability = match previous {
            Some(previous) => ProjectSourceFilesAvailability::Stale { previous },
            None => ProjectSourceFilesAvailability::Loading,
        };
        self.set_project_source_files_availability(availability);
    }

    fn set_project_source_files_availability(
        &mut self,
        availability: ProjectSourceFilesAvailability,
    ) {
        let state = self.project_loading_state();
        state.set_source_files(self).to(availability);
    }
}

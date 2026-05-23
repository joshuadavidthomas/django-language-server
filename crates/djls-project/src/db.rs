use salsa::Setter;

use crate::project::Project;
use crate::root_discovery::ProjectRootDiscovery;
use crate::source_files::SourceFileInventory;

#[salsa::db]
pub trait Db: djls_source::Db {
    fn project(&self) -> Project;

    fn set_source_file_inventory(&mut self, inventory: SourceFileInventory) {
        let project = self.project();
        if project.source_inventory(self) != inventory {
            project.set_source_inventory(self).to(inventory);
        }
    }

    fn set_project_root_discovery(&mut self, discovery: ProjectRootDiscovery) {
        let project = self.project();
        if project.root_discovery(self) != &discovery {
            project.set_root_discovery(self).to(discovery);
        }
    }
}

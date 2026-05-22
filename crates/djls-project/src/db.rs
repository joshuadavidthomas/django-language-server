use salsa::Setter;

use crate::Project;
use crate::ProjectDiscovery;
use crate::ProjectSourceInventory;

#[salsa::db]
pub trait Db: djls_source::Db {
    fn project(&self) -> Project;

    fn set_project_source_inventory(&mut self, inventory: ProjectSourceInventory) {
        let project = self.project();
        if project.source_inventory(self) != inventory {
            project.set_source_inventory(self).to(inventory);
        }
    }

    fn set_project_discovery(&mut self, discovery: ProjectDiscovery) {
        let project = self.project();
        if project.discovery(self) != &discovery {
            project.set_discovery(self).to(discovery);
        }
    }
}

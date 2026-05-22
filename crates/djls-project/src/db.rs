use salsa::Setter;

use crate::Project;
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
}

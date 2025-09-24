use djls_templates::Node;
use djls_templates::NodeList;

use crate::Db;

/// Semantic model builder that operates on Django template nodelists.
///
/// This trait defines the interface for building semantic models from Django templates.
/// A semantic model is any representation that captures some aspect of the template's
/// meaning - structure, dependencies, types, security properties, etc.
pub trait SemanticModel<'db> {
    type Model;

    /// Build the semantic model from a nodelist
    #[allow(dead_code)] // use is gated behind cfg(test) for now
    fn model(mut self, db: &'db dyn Db, nodelist: NodeList<'db>) -> Self::Model
    where
        Self: Sized,
    {
        for node in nodelist.nodelist(db).iter().cloned() {
            self.observe(node);
        }
        self.construct()
    }

    /// Observe a single node during traversal and extract semantic information
    #[allow(dead_code)] // use is gated behind cfg(test) for now
    fn observe(&mut self, node: Node);

    /// Construct the final semantic model from observed semantics
    #[allow(dead_code)] // use is gated behind cfg(test) for now
    fn construct(self) -> Self::Model;
}

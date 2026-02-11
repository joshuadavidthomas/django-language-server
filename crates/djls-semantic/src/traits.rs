use djls_templates::visitor::Visitor;
use djls_templates::NodeList;

use crate::Db;

/// Semantic model builder that operates on Django template nodelists.
///
/// This trait defines the interface for building semantic models from Django templates.
/// A semantic model is any representation that captures some aspect of the template's
/// meaning - structure, dependencies, types, security properties, etc.
pub trait SemanticModel<'db>: Visitor {
    type Model;

    /// Build the semantic model from a nodelist
    fn model(mut self, db: &'db dyn Db, nodelist: NodeList<'db>) -> Self::Model
    where
        Self: Sized,
    {
        for node in nodelist.nodelist(db) {
            self.visit_node(node);
        }
        self.construct()
    }

    /// Construct the final semantic model from observed semantics
    fn construct(self) -> Self::Model;
}

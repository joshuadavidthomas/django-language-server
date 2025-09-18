use djls_templates::{Node, NodeList};

use crate::Db;

/// Semantic model builder that operates on Django template nodelists.
///
/// This trait defines the interface for building semantic models from Django templates.
/// A semantic model is any representation that captures some aspect of the template's
/// meaning - structure, dependencies, types, security properties, etc.
pub trait SemanticModel<'db> {
    /// The semantic model this builder produces
    type Model;

    /// Build the semantic model from a nodelist
    fn model(mut self, db: &'db dyn Db, nodelist: NodeList<'db>) -> Self::Model
    where
        Self: Sized,
    {
        // Default implementation: traverse and observe all nodes
        for node in nodelist.nodelist(db).iter().cloned() {
            self.observe(node);
        }
        self.construct()
    }

    /// Observe a single node during traversal and extract semantic information
    fn observe(&mut self, node: Node<'db>);

    /// Construct the final semantic model from observed semantics
    fn construct(self) -> Self::Model;
}

// Example implementations showing how different semantic models can be built
// from the same Django template source
//
// Example usage:
// ```
// use djls_semantic::blocks::{BlockModelBuilder, SemanticModel};
//
// // Build a block structure model
// let block_tree = BlockModelBuilder::new(db, shapes)
//     .model(db, nodelist);
//
// // Future: Build other semantic models from the same template
// // let inheritance = InheritanceModelBuilder::new(db)
// //     .model(db, nodelist);
// //
// // let dependencies = DependencyModelBuilder::new(db)
// //     .model(db, nodelist);
// ```

/*
// Example future implementations:
/// Builder for extracting template inheritance relationships
pub struct InheritanceTreeBuilder<'db> {
    db: &'db dyn Db,
    extends_from: Option<String>,
    blocks: HashMap<String, BlockInfo>,
}

impl<'db> TemplateTreeBuilder<'db> for InheritanceTreeBuilder<'db> {
    type Output = InheritanceTree;

    fn handle_node(&mut self, node: Node<'db>) {
        if let Node::Tag { name, bits, .. } = node {
            match name.text(self.db) {
                "extends" => {
                    // Extract parent template name
                    if let Some(first_bit) = bits.first() {
                        self.extends_from = Some(first_bit.text(self.db).to_string());
                    }
                }
                "block" => {
                    // Track block definitions
                    if let Some(block_name) = bits.first() {
                        self.blocks.insert(
                            block_name.text(self.db).to_string(),
                            BlockInfo { /* ... */ }
                        );
                    }
                }
                _ => {} // Ignore other tags
            }
        }
    }

    fn finalize(self) -> InheritanceTree {
        InheritanceTree {
            extends_from: self.extends_from,
            blocks: self.blocks,
        }
    }
}

/// Builder for extracting variable dependencies
pub struct VariableDependencyBuilder<'db> {
    db: &'db dyn Db,
    variables: HashSet<String>,
    filters: HashMap<String, Vec<String>>,
}

impl<'db> TemplateTreeBuilder<'db> for VariableDependencyBuilder<'db> {
    type Output = VariableDependencyMap;

    fn handle_node(&mut self, node: Node<'db>) {
        if let Node::Variable { var, filters, .. } = node {
            self.variables.insert(var.text(self.db).to_string());
            // Track filter usage
            for filter in filters {
                self.filters
                    .entry(var.text(self.db).to_string())
                    .or_default()
                    .push(filter.text(self.db).to_string());
            }
        }
    }

    fn finalize(self) -> VariableDependencyMap {
        VariableDependencyMap {
            variables: self.variables,
            filters: self.filters,
        }
    }
}
*/


pub mod loads;
pub mod symbols;

use djls_templates::Node;
use djls_templates::NodeList;
pub use loads::parse_load_bits;
pub use loads::LoadKind;
pub use loads::LoadStatement;
pub use loads::LoadedLibraries;
pub use symbols::AvailableSymbols;
pub use symbols::FilterAvailability;
pub use symbols::TagAvailability;

use crate::db::Db;

/// Compute the [`LoadedLibraries`] for a parsed template's node list.
///
/// Iterates all nodes, identifies `{% load %}` tags, parses each into a
/// [`LoadStatement`], and returns an ordered [`LoadedLibraries`] collection
/// that supports position-aware availability queries.
///
/// Cached by Salsa — re-computes only when the underlying [`NodeList`] changes.
#[salsa::tracked]
pub fn compute_loaded_libraries(db: &dyn Db, nodelist: NodeList<'_>) -> LoadedLibraries {
    let statements: Vec<LoadStatement> = nodelist
        .nodelist(db)
        .iter()
        .filter_map(|node| match node {
            Node::Tag {
                name, bits, span, ..
            } if name == "load" => {
                let kind = parse_load_bits(bits)?;
                Some(LoadStatement::new(*span, kind))
            }
            _ => None,
        })
        .collect();

    LoadedLibraries::new(statements)
}

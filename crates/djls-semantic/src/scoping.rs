pub mod loads;
pub mod symbols;

use djls_templates::Node;
use djls_templates::NodeList;
pub use loads::parse_load_bits;
pub use loads::LoadKind;
pub use loads::LoadState;
pub use loads::LoadStatement;
pub use loads::LoadedLibraries;
pub use symbols::AvailableSymbols;
pub use symbols::FilterAvailability;
pub use symbols::SymbolIndex;
pub use symbols::TagAvailability;

use crate::db::Db;

/// Compute the [`LoadedLibraries`] for a parsed template's node list.
#[salsa::tracked(returns(ref))]
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

/// Compute a [`SymbolIndex`] for position-based symbol availability lookups.
#[salsa::tracked(returns(ref))]
pub fn compute_symbol_index(db: &dyn Db, nodelist: NodeList<'_>) -> SymbolIndex {
    let loaded_libraries = compute_loaded_libraries(db, nodelist);
    let template_libraries = db.template_libraries();
    SymbolIndex::build(loaded_libraries, template_libraries)
}

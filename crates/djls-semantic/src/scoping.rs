pub(crate) mod loads;
pub(crate) mod symbols;

use djls_templates::Node;
use djls_templates::NodeList;

use crate::db::Db;
pub use crate::scoping::loads::LoadKind;
pub(crate) use crate::scoping::loads::LoadState;
pub(crate) use crate::scoping::loads::LoadStatement;
pub(crate) use crate::scoping::loads::LoadedLibraries;
pub use crate::scoping::symbols::AvailableSymbols;
pub(crate) use crate::scoping::symbols::SymbolIndex;

/// Compute the [`LoadedLibraries`] for a parsed template's node list.
#[salsa::tracked(returns(ref))]
pub(crate) fn compute_loaded_libraries(db: &dyn Db, nodelist: NodeList<'_>) -> LoadedLibraries {
    let statements: Vec<LoadStatement> = nodelist
        .nodelist(db)
        .iter()
        .filter_map(|node| match node {
            Node::Tag {
                name, bits, span, ..
            } => LoadStatement::from_tag(name, bits, *span),
            _ => None,
        })
        .collect();

    LoadedLibraries::new(statements)
}

/// Compute available symbols at a byte position in a parsed template.
#[salsa::tracked]
pub fn available_symbols_at(
    db: &dyn Db,
    nodelist: NodeList<'_>,
    position: u32,
) -> AvailableSymbols {
    compute_symbol_index(db, nodelist)
        .symbols_at(position)
        .clone()
}

/// Compute a [`SymbolIndex`] for position-based symbol availability lookups.
#[salsa::tracked(returns(ref))]
pub(crate) fn compute_symbol_index(db: &dyn Db, nodelist: NodeList<'_>) -> SymbolIndex {
    let loaded_libraries = compute_loaded_libraries(db, nodelist);
    let template_libraries = db.template_libraries();
    SymbolIndex::build(loaded_libraries, template_libraries)
}

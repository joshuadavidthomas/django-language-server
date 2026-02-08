mod types;

#[cfg(feature = "parser")]
mod scan;

#[cfg(feature = "parser")]
pub use scan::scan_environment;
#[cfg(feature = "parser")]
pub use scan::scan_environment_with_symbols;
pub use types::EnvironmentInventory;
pub use types::EnvironmentLibrary;
pub use types::EnvironmentSymbol;

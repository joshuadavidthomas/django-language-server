//! Static Extraction for Django project facts.
//!
//! The extraction boundary is deliberately pure: callers provide Python source
//! text and answer star-import recursion through `StarImportResolver`. This
//! module does not read files, resolve search paths, or depend on Salsa.

mod extractor;
mod paths;
mod settings;

//! Browser entry points: an in-memory Django project analyzed with the production database,
//! driven either file by file (`Workspace`) or as an LSP server (`LanguageServer`).

mod project;
mod server;
mod workspace;

pub use server::LanguageServer;
use wasm_bindgen::prelude::*;
pub use workspace::Workspace;

/// Supply a Django release this build does not embed: a Django wheel or a vendored bundle zip.
///
/// Returns its feature line, such as `"5.2"`. Register before any `Workspace` or
/// `LanguageServer` uses that release.
#[wasm_bindgen(js_name = registerDjango)]
pub fn register_django(archive: Vec<u8>) -> Result<String, JsError> {
    Ok(djls_project::register_django_bundle(archive)?
        .as_str()
        .to_owned())
}

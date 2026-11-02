//! Browser entry point: a synchronous LSP server (`LanguageServer`) over an in-memory
//! Django project, analyzed with the production database.

mod project;
mod server;

pub use server::LanguageServer;
use wasm_bindgen::prelude::*;

/// Supply a Django release this build does not embed: a Django wheel or a vendored bundle zip.
///
/// Returns its feature line, such as `"5.2"`. Register before any `LanguageServer` uses that
/// release.
#[wasm_bindgen(js_name = registerDjango)]
pub fn register_django(archive: Vec<u8>) -> Result<String, JsError> {
    Ok(djls_project::register_django_bundle(archive)?
        .as_str()
        .to_owned())
}

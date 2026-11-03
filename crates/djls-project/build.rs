use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use sha2::Digest;
use sha2::Sha256;

/// Hash each vendored Django archive once here instead of at every startup.
/// The digest names the bundle's on-disk cache directory.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let vendor = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("vendor");
    println!("cargo:rerun-if-changed={}", vendor.display());

    for entry in fs::read_dir(&vendor)? {
        let path = entry?.path();
        if path.extension().is_none_or(|extension| extension != "zip") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or("vendored archive name is not UTF-8")?;
        let mut digest = String::with_capacity(64);
        for byte in Sha256::digest(fs::read(&path)?) {
            write!(&mut digest, "{byte:02x}")?;
        }
        let name = stem.to_uppercase().replace(['-', '.'], "_");
        println!("cargo:rustc-env={name}_ZIP_SHA256={digest}");
    }
    Ok(())
}

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");

    let workspace_dir = env::var("CARGO_WORKSPACE_DIR")?;
    let djls_cargo_toml = PathBuf::from(workspace_dir)
        .join("crates")
        .join("djls")
        .join("Cargo.toml");

    println!("cargo:rerun-if-changed={}", djls_cargo_toml.display());

    let contents = fs::read_to_string(&djls_cargo_toml)?;
    let cargo_toml: toml::Value = toml::from_str(&contents)?;

    let version = cargo_toml
        .get("package")
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .ok_or("Failed to extract version from djls Cargo.toml")?;

    println!("cargo:rustc-env=DJLS_VERSION={version}");
    Ok(())
}

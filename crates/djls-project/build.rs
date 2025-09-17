use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let manifest_path = PathBuf::from(manifest_dir);
    let inspector_dir = manifest_path.join("inspector");
    let dist_dir = manifest_path.join("dist");
    let pyz_path = dist_dir.join("djls_inspector.pyz");

    println!("cargo:rerun-if-changed={}", inspector_dir.display());

    std::fs::create_dir_all(&dist_dir)
        .expect("Failed to create inspector/dist directory for Python zipapp");

    let python = which::which("python3")
        .or_else(|_| which::which("python"))
        .expect("Python not found. Please install Python to build this project.");
    println!(
        "cargo:warning=Building Python inspector with: {}",
        python.display()
    );

    let output = Command::new(&python)
        .arg("-m")
        .arg("zipapp")
        .arg(&inspector_dir)
        .arg("-o")
        .arg(&pyz_path)
        .arg("-c")
        .output()
        .expect("Failed to run Python zipapp builder");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        panic!("Failed to build Python inspector:\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}");
    }

    assert!(
        pyz_path.exists(),
        "Python inspector zipapp was not created at expected location: {}",
        pyz_path.display()
    );

    let metadata =
        std::fs::metadata(&pyz_path).expect("Failed to get metadata for inspector zipapp");

    println!(
        "cargo:warning=Successfully built Python inspector: {} ({} bytes)",
        pyz_path.display(),
        metadata.len()
    );
}

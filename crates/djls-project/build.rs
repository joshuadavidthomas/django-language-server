/// Build script that automatically compiles the Python inspector zipapp.
/// 
/// This ensures the Python inspector is always up-to-date when building
/// the Rust project. The inspector is a Python zipapp that runs in a 
/// subprocess to handle Django-specific operations via IPC.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    // Tell Cargo to rerun this build script if the Python source changes
    println!("cargo:rerun-if-changed=../../python/src/djls_inspector");
    println!("cargo:rerun-if-changed=../../python/build.py");
    
    // Get workspace directory
    let workspace_dir = env::var("CARGO_WORKSPACE_DIR")
        .expect("CARGO_WORKSPACE_DIR not set");
    let workspace_path = PathBuf::from(workspace_dir);
    let python_dir = workspace_path.join("python");
    let dist_dir = python_dir.join("dist");
    let pyz_path = dist_dir.join("djls_inspector.pyz");
    
    // Create dist directory if it doesn't exist
    std::fs::create_dir_all(&dist_dir)
        .expect("Failed to create python/dist directory");
    
    // Find Python executable - try python3 first, then python
    let python = which::which("python3")
        .or_else(|_| which::which("python"))
        .expect("Python not found. Please install Python to build this project.");
    
    println!("cargo:warning=Building Python inspector with: {}", python.display());
    
    // Run the Python build script
    let output = Command::new(&python)
        .arg("build.py")
        .current_dir(&python_dir)
        .output()
        .expect("Failed to run Python build script");
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        panic!(
            "Failed to build Python inspector:\nSTDOUT:\n{}\nSTDERR:\n{}", 
            stdout, stderr
        );
    }
    
    // Verify the pyz file was created
    if !pyz_path.exists() {
        panic!(
            "Python inspector zipapp was not created at expected location: {:?}", 
            pyz_path
        );
    }
    
    // Get file size for informational purposes
    let metadata = std::fs::metadata(&pyz_path)
        .expect("Failed to get metadata for inspector zipapp");
    
    println!(
        "cargo:warning=Successfully built Python inspector: {} ({} bytes)", 
        pyz_path.display(),
        metadata.len()
    );
}
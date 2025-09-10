use std::env;
use std::path::PathBuf;
use std::process::Command;

fn get_python_executable_path() -> PathBuf {
    let python = which::which("python3")
        .or_else(|_| which::which("python"))
        .expect("Python not found. Please install Python to build this project.");
    println!(
        "cargo:warning=Building Python inspector with: {}",
        python.display()
    );
    python
}

fn main() {
    println!("cargo:rerun-if-changed=../../python/src/djls_inspector");
    println!("cargo:rerun-if-changed=../../python/build.py");

    let workspace_dir = env::var("CARGO_WORKSPACE_DIR").expect("CARGO_WORKSPACE_DIR not set");
    let workspace_path = PathBuf::from(workspace_dir);
    let python_dir = workspace_path.join("python");
    let dist_dir = python_dir.join("dist");
    let pyz_path = dist_dir.join("djls_inspector.pyz");

    std::fs::create_dir_all(&dist_dir).expect("Failed to create python/dist directory");

    let python = get_python_executable_path();

    let output = Command::new(&python)
        .arg("build.py")
        .current_dir(&python_dir)
        .output()
        .expect("Failed to run Python build script");

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

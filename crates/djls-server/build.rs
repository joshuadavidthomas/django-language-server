/// Build script to configure linking against the Python library.
///
/// This script is necessary when the crate's code, particularly tests
/// that interact with the Python interpreter via `pyo3` (e.g., using
/// `Python::with_gil`, importing modules, calling Python functions),
/// needs symbols from the Python C API at link time.
///
/// It uses `pyo3-build-config` to detect the Python installation and
/// prints the required `cargo:rustc-link-search` and `cargo:rustc-link-lib`
/// directives to Cargo, enabling the linker to find and link against the
/// appropriate Python library (e.g., libpythonX.Y.so).
///
/// It also adds an RPATH linker argument on Unix-like systems so the
/// resulting test executable can find the Python library at runtime.
///
/// Note: Each crate whose test target requires Python linking needs its
/// own `build.rs` with this logic.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Set up #[cfg] flags first (useful for conditional compilation)
    pyo3_build_config::use_pyo3_cfgs();

    // Get the Python interpreter configuration directly
    let config = pyo3_build_config::get();

    // Add the library search path if available
    if let Some(lib_dir) = &config.lib_dir {
        println!("cargo:rustc-link-search=native={}", lib_dir);

        // Add RPATH linker argument for Unix-like systems (Linux, macOS)
        // This helps the test executable find the Python library at runtime.
        #[cfg(not(windows))]
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir);
    } else {
        println!("cargo:warning=Python library directory not found in config.");
    }

    // Add the library link directive if available
    if let Some(lib_name) = &config.lib_name {
        println!("cargo:rustc-link-lib=dylib={}", lib_name);
    } else {
        println!("cargo:warning=Python library name not found in config.");
    }
}

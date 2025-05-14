/// Sets up the necessary Cargo directives for linking against the Python library.
///
/// This function should be called from the `build.rs` script of any crate
/// within the workspace whose compiled artifact (e.g., test executable, binary)
/// needs to link against the Python C API at compile time and find the
/// corresponding shared library at runtime. This is typically required for
/// crates using `pyo3` features like `Python::with_gil` or defining `#[pyfunction]`s
/// directly in their test or binary code.
///
/// It uses `pyo3-build-config` to detect the active Python installation and
/// prints the required `cargo:rustc-link-search` and `cargo:rustc-link-lib`
/// directives to Cargo, enabling the linker to find and link against the
/// appropriate Python library (e.g., libpythonX.Y.so).
///
/// It also adds an RPATH linker argument on Unix-like systems (`-Wl,-rpath,...`)
/// to help the resulting executable find the Python shared library at runtime
/// without needing manual `LD_LIBRARY_PATH` adjustments in typical scenarios.
pub fn setup_python_linking() {
    // Instruct Cargo to rerun the calling build script if the Python config changes.
    // Using PYO3_CONFIG_FILE is a reliable way to detect changes in the active Python env.
    println!("cargo:rerun-if-changed=build.rs");

    // Set up #[cfg] flags first (useful for conditional compilation)
    pyo3_build_config::use_pyo3_cfgs();

    // Get the Python interpreter configuration directly
    let config = pyo3_build_config::get();

    let is_extension_module = std::env::var("CARGO_FEATURE_EXTENSION_MODULE").is_ok();

    // Only link libpython explicitly if we are NOT building an extension module.
    if !is_extension_module {
        if let Some(lib_name) = &config.lib_name {
            println!("cargo:rustc-link-lib=dylib={lib_name}");
        } else {
            // Warn only if linking is actually needed but we can't find the lib name
            println!("cargo:warning=Python library name not found in config (needed for non-extension module
builds).");
        }
    }

    // Add the library search path and RPATH if available.
    // These are needed for test executables and potential future standalone binaries,
    // and generally harmless for extension modules.
    if let Some(lib_dir) = &config.lib_dir {
        println!("cargo:rustc-link-search=native={lib_dir}");
        #[cfg(not(windows))]
        println!("cargo:rustc-link-arg=-Wl,-rpath,{lib_dir}");
    } else {
        // Warn only if linking is actually needed but we can't find the lib dir
        if !is_extension_module {
            println!("cargo:warning=Python library directory not found in config.");
        }
    }
}

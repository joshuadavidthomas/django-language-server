/// `PyO3` entrypoint for the Django Language Server CLI.
///
/// This module provides a Python interface using `PyO3` to solve Python runtime
/// interpreter linking issues. The `PyO3` approach avoids complexities with
/// static/dynamic linking when building binaries that interact with Python.
mod args;
mod cli;
mod commands;
mod exit;

use std::env;

use pyo3::prelude::*;

#[pyfunction]
/// Entry point called by Python when the CLI is invoked.
/// This function handles argument parsing from Python and routes to the Rust CLI logic.
fn entrypoint(_py: Python) -> PyResult<()> {
    // Skip python interpreter and script path, add command name
    let args: Vec<String> = std::iter::once("djls".to_string())
        .chain(env::args().skip(2))
        .collect();

    // Run the CLI with the adjusted args
    cli::run(args).map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

    Ok(())
}

#[pymodule]
fn djls(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(entrypoint, m)?)?;
    Ok(())
}

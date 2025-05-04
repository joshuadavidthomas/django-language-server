mod args;
mod cli;
mod commands;

use pyo3::prelude::*;
use std::env;
use std::process::ExitCode;

#[pyfunction]
fn entrypoint(_py: Python) -> PyResult<()> {
    // Skip python interpreter and script path, add command name
    let args: Vec<String> = std::iter::once("djls".to_string())
        .chain(env::args().skip(2))
        .collect();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let result = runtime.block_on(cli::run(args));

    match result {
        Ok(code) => {
            if code != ExitCode::SUCCESS {
                std::process::exit(1);
            }
            Ok(())
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            if let Some(source) = e.source() {
                eprintln!("Caused by: {}", source);
            }
            std::process::exit(1);
        }
    }
}

#[pymodule]
fn djls(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(entrypoint, m)?)?;
    Ok(())
}

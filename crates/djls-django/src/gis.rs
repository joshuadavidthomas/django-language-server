use crate::django::Apps;
use djls_python::{Python, RunnerError};
use std::process::Command;

pub fn check_gis_setup(py: &Python) -> Result<bool, GISError> {
    let has_geodjango = Apps::check_installed(py, "django.contrib.gis")?;
    let gdal_is_installed = Command::new("gdalinfo")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);

    Ok(!has_geodjango || gdal_is_installed)
}

#[derive(Debug, thiserror::Error)]
pub enum GISError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Runner(#[from] RunnerError),
}

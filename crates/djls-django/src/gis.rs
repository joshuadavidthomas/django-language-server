use crate::apps::Apps;
use djls_ipc::{PythonProcess, TransportError};
use std::process::Command;

pub fn check_gis_setup(python: &mut PythonProcess) -> Result<bool, GISError> {
    let has_geodjango = Apps::check_installed(python, "django.contrib.gis")?;
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

    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),
}

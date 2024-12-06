use djls_python::Python;
use std::{process::Command, sync::Arc};

pub fn gdal_is_installed() -> bool {
    Command::new("gdalinfo")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

pub fn has_geodjango(py: Arc<Python>) -> Result<bool, GISError> {
    let check_result = py.run_python(
        r#"
import json
from django.conf import settings
print(json.dumps({
    'has_geodjango': 'django.contrib.gis' in settings.INSTALLED_APPS
}))
        "#,
    )?;

    let status: serde_json::Value = serde_json::from_str(&check_result)?;

    Ok(status["has_geodjango"].as_bool().unwrap())
}

#[derive(Debug, thiserror::Error)]
pub enum GISError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),
}

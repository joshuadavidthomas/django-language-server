use std::io::Write;

use anyhow::Context;
use anyhow::Result;
use camino::Utf8Path;
use tempfile::NamedTempFile;

const INSPECTOR_PYZ: &[u8] = include_bytes!(concat!(
    env!("CARGO_WORKSPACE_DIR"),
    "/python/dist/djls_inspector.pyz"
));

pub struct InspectorFile(NamedTempFile);

impl InspectorFile {
    pub fn create() -> Result<Self> {
        let mut zipapp_file = tempfile::Builder::new()
            .prefix("djls_inspector_")
            .suffix(".pyz")
            .tempfile()
            .context("Failed to create temp file for inspector")?;

        zipapp_file
            .write_all(INSPECTOR_PYZ)
            .context("Failed to write inspector zipapp to temp file")?;
        zipapp_file
            .flush()
            .context("Failed to flush inspector zipapp")?;

        Ok(Self(zipapp_file))
    }

    pub fn path(&self) -> &Utf8Path {
        Utf8Path::from_path(self.0.path()).expect("Temp file path should always be valid UTF-8")
    }
}

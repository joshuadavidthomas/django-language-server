//! Filesystem cache for inspector responses.
//!
//! Caches the `TemplateLibrariesResponse` from the Python inspector subprocess
//! to avoid blocking startup on Python process spawn + Django import. The cache
//! is keyed by a hash of the project environment (root, interpreter, settings
//! module, pythonpath) and stamped with the djls version to avoid stale data
//! after upgrades.
//!
//! The cache is best-effort: startup always kicks off a real inspector query in
//! the background. The cache just provides data to work with while waiting.

use std::fs;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;

use crate::Interpreter;
use crate::TemplateLibrariesResponse;

/// Envelope wrapping a cached inspector response with version metadata.
#[derive(Serialize, Deserialize)]
struct CacheEnvelope {
    /// djls version that wrote this cache entry.
    djls_version: String,
    /// The cached inspector response.
    response: TemplateLibrariesResponse,
}

/// Compute a hex-encoded SHA-256 hash of the project environment.
///
/// The cache key is derived from the inputs that determine the inspector's
/// output: project root, interpreter specification, Django settings module,
/// and PYTHONPATH entries.
fn cache_key(
    root: &Utf8Path,
    interpreter: &Interpreter,
    django_settings_module: Option<&str>,
    pythonpath: &[String],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(root.as_str().as_bytes());
    hasher.update(b"\0");
    hasher.update(format!("{interpreter:?}").as_bytes());
    hasher.update(b"\0");
    hasher.update(django_settings_module.unwrap_or("").as_bytes());
    hasher.update(b"\0");
    for path in pythonpath {
        hasher.update(path.as_bytes());
        hasher.update(b"\0");
    }
    format!("{:x}", hasher.finalize())
}

/// Resolve the cache directory for a given project environment.
fn cache_dir(
    root: &Utf8Path,
    interpreter: &Interpreter,
    django_settings_module: Option<&str>,
    pythonpath: &[String],
) -> Option<Utf8PathBuf> {
    let base = djls_conf::project_dirs()
        .and_then(|dirs| Utf8PathBuf::from_path_buf(dirs.cache_dir().to_path_buf()).ok())?;
    let key = cache_key(root, interpreter, django_settings_module, pythonpath);
    Some(base.join("inspector").join(&key[..16]))
}

/// Load a cached inspector response from disk.
///
/// Returns `None` if the cache file doesn't exist, is corrupt, or was written
/// by a different djls version.
pub fn load_cached_inspector_response(
    root: &Utf8Path,
    interpreter: &Interpreter,
    django_settings_module: Option<&str>,
    pythonpath: &[String],
) -> Option<TemplateLibrariesResponse> {
    let dir = cache_dir(root, interpreter, django_settings_module, pythonpath)?;
    let path = dir.join("inspector.json");

    let content = fs::read_to_string(path.as_std_path()).ok()?;
    let envelope: CacheEnvelope = serde_json::from_str(&content).ok()?;

    if envelope.djls_version != env!("CARGO_PKG_VERSION") {
        tracing::debug!(
            "Inspector cache version mismatch: cached={}, current={}",
            envelope.djls_version,
            env!("CARGO_PKG_VERSION"),
        );
        return None;
    }

    tracing::info!("Loaded inspector response from cache: {}", path);
    Some(envelope.response)
}

/// Write an inspector response to the filesystem cache.
///
/// Best-effort: logs warnings on failure but never panics.
pub fn save_inspector_response(
    root: &Utf8Path,
    interpreter: &Interpreter,
    django_settings_module: Option<&str>,
    pythonpath: &[String],
    response: &TemplateLibrariesResponse,
) {
    let Some(dir) = cache_dir(root, interpreter, django_settings_module, pythonpath) else {
        return;
    };

    let Ok(response_value) = serde_json::to_value(response) else {
        tracing::warn!("Failed to serialize inspector response for cache");
        return;
    };
    let Ok(response_copy) = serde_json::from_value(response_value) else {
        tracing::warn!("Failed to roundtrip inspector response for cache");
        return;
    };
    let envelope = CacheEnvelope {
        djls_version: env!("CARGO_PKG_VERSION").to_string(),
        response: response_copy,
    };

    if let Err(e) = fs::create_dir_all(dir.as_std_path()) {
        tracing::warn!("Failed to create inspector cache directory: {e}");
        return;
    }

    let path = dir.join("inspector.json");
    match serde_json::to_string(&envelope) {
        Ok(json) => {
            if let Err(e) = fs::write(path.as_std_path(), json) {
                tracing::warn!("Failed to write inspector cache: {e}");
            } else {
                tracing::debug!("Saved inspector response to cache: {}", path);
            }
        }
        Err(e) => {
            tracing::warn!("Failed to serialize inspector cache: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_response() -> TemplateLibrariesResponse {
        TemplateLibrariesResponse {
            symbols: vec![],
            libraries: std::collections::BTreeMap::from([(
                "i18n".to_string(),
                "django.templatetags.i18n".to_string(),
            )]),
            builtins: vec!["django.template.defaulttags".to_string()],
        }
    }

    #[test]
    fn cache_key_deterministic() {
        let root = Utf8Path::new("/project");
        let interpreter = Interpreter::VenvPath("/project/.venv".to_string());
        let dsm = Some("myproject.settings");
        let pythonpath = vec!["/extra".to_string()];

        let key1 = cache_key(root, &interpreter, dsm, &pythonpath);
        let key2 = cache_key(root, &interpreter, dsm, &pythonpath);
        assert_eq!(key1, key2);
    }

    #[test]
    fn cache_key_varies_with_inputs() {
        let interpreter = Interpreter::VenvPath("/project/.venv".to_string());
        let pythonpath: Vec<String> = vec![];

        let key1 = cache_key(Utf8Path::new("/project-a"), &interpreter, None, &pythonpath);
        let key2 = cache_key(Utf8Path::new("/project-b"), &interpreter, None, &pythonpath);
        assert_ne!(key1, key2);
    }

    #[test]
    fn roundtrip_through_filesystem() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let interpreter = Interpreter::VenvPath("/test/.venv".to_string());

        let response = test_response();

        save_inspector_response(&root, &interpreter, None, &[], &response);
        let loaded = load_cached_inspector_response(&root, &interpreter, None, &[]);

        // Cache reads from the XDG dir, not from the project root — so this
        // only works if project_dirs() resolves. If it doesn't (CI), the
        // save is a no-op and load returns None.
        if djls_conf::project_dirs().is_some() {
            let loaded = loaded.expect("should load cached response");
            assert_eq!(loaded.libraries.len(), 1);
            assert_eq!(loaded.builtins.len(), 1);
        }
    }
}

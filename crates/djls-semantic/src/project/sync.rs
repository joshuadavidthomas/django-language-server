//! Synchronize external project state into Salsa inputs.
//!
//! This module is the imperative boundary for project data. It may ask
//! Django, Python, and the filesystem for facts, then writes changed facts to
//! the `Project` input. Pure semantic derivation stays in tracked queries.

use std::fmt::Write;
use std::fs;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::Utf8PathClean;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;
use salsa::Setter;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;

use crate::project::db::Db as ProjectDb;
use crate::project::input::Project;
use crate::project::input::ProjectTemplateFiles;
use crate::project::input::TemplateDirs;
use crate::project::introspector::IntrospectionRequest;
use crate::project::python::Interpreter;
use crate::project::resolve::model_modules;
use crate::project::resolve::templatetag_modules;
use crate::project::symbols::TemplateLibrarySnapshot;

/// Refresh all external project data.
///
/// This is the imperative boundary between the outside world and Salsa inputs:
/// it asks Django/Python/the filesystem for current facts, writes changed facts
/// into the `Project` input, then lets tracked semantic queries handle editor
/// file contents and downstream derivations.
pub fn refresh_external_data(db: &mut dyn ProjectDb) {
    let Some(project) = db.project() else {
        return;
    };

    project.refresh_source_roots(db);
    refresh_template_dirs(db, project);
    refresh_template_libraries(db, project);
    refresh_template_files(db, project);
    refresh_python_modules(db, project);
}

/// Populate template libraries from the filesystem cache, if available.
///
/// This is a fast, synchronous startup path. It gives completions and
/// diagnostics previously discovered library data while fresh project
/// introspection runs in the background.
pub fn load_template_library_cache(db: &mut dyn ProjectDb) -> bool {
    let Some(project) = db.project() else {
        return false;
    };

    let interpreter = project.interpreter(db).clone();
    let root = project.root(db).clone();
    let django_settings_module = project.django_settings_module(db).clone();
    let pythonpath = project.pythonpath(db).clone();
    let Some(dir) = cache_dir(
        &root,
        &interpreter,
        django_settings_module.as_deref(),
        &pythonpath,
    ) else {
        return false;
    };
    // Keep the legacy filename for on-disk cache compatibility.
    let path = dir.join("inspector.json");

    let Ok(content) = fs::read_to_string(path.as_std_path()) else {
        return false;
    };
    let Ok(envelope) = serde_json::from_str::<CacheEnvelope>(&content) else {
        return false;
    };

    if envelope.djls_version != env!("CARGO_PKG_VERSION") {
        tracing::debug!(
            "Template library snapshot cache version mismatch: cached={}, current={}",
            envelope.djls_version,
            env!("CARGO_PKG_VERSION"),
        );
        return false;
    }

    tracing::info!("Loaded template library snapshot from cache: {}", path);
    apply_template_library_snapshot(db, project, envelope.response);
    true
}

#[derive(Serialize)]
struct TemplateDirsRequest;

#[derive(Deserialize)]
struct TemplateDirsResponse {
    dirs: Vec<Utf8PathBuf>,
}

impl IntrospectionRequest for TemplateDirsRequest {
    const NAME: &'static str = "template_dirs";
    type Response = TemplateDirsResponse;
}

fn refresh_template_dirs(db: &mut dyn ProjectDb, project: Project) {
    tracing::debug!("Requesting template directories from project introspection");

    let Some(response) = db.project_introspector().query(db, &TemplateDirsRequest) else {
        return;
    };

    let dir_count = response.dirs.len();
    tracing::info!(
        "Retrieved {} template directories from project introspection",
        dir_count
    );

    for (i, dir) in response.dirs.iter().enumerate() {
        tracing::debug!("  Template dir [{}]: {}", i, dir);
    }

    let missing_dirs: Vec<_> = response
        .dirs
        .iter()
        .filter(|dir| !db.path_exists(dir))
        .collect();

    if !missing_dirs.is_empty() {
        tracing::warn!(
            "Found {} non-existent template directories: {:?}",
            missing_dirs.len(),
            missing_dirs
        );
    }

    let next = TemplateDirs::Known(response.dirs);
    if project.template_dirs(db) != &next {
        project.set_template_dirs(db).to(next);
    }
}

#[derive(Serialize)]
struct TemplateLibrarySnapshotRequest;

impl IntrospectionRequest for TemplateLibrarySnapshotRequest {
    const NAME: &'static str = "template_libraries";
    type Response = TemplateLibrarySnapshot;
}

fn refresh_template_libraries(db: &mut dyn ProjectDb, project: Project) {
    let Some(snapshot) = db
        .project_introspector()
        .query(db, &TemplateLibrarySnapshotRequest)
    else {
        return;
    };

    let interpreter = project.interpreter(db).clone();
    let root = project.root(db).clone();
    let django_settings_module = project.django_settings_module(db).clone();
    let pythonpath = project.pythonpath(db).clone();
    if let Some(dir) = cache_dir(
        &root,
        &interpreter,
        django_settings_module.as_deref(),
        &pythonpath,
    ) {
        let envelope = CacheEnvelope {
            djls_version: env!("CARGO_PKG_VERSION").to_string(),
            response: snapshot.clone(),
        };

        if let Err(e) = fs::create_dir_all(dir.as_std_path()) {
            tracing::warn!("Failed to create template library snapshot cache directory: {e}");
        } else {
            let path = dir.join("inspector.json");
            match serde_json::to_string(&envelope) {
                Ok(json) => {
                    if let Err(e) = fs::write(path.as_std_path(), json) {
                        tracing::warn!("Failed to write template library snapshot cache: {e}");
                    } else {
                        tracing::debug!("Saved template library snapshot to cache: {}", path);
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to serialize template library snapshot cache: {e}");
                }
            }
        }
    }

    apply_template_library_snapshot(db, project, snapshot);
}

fn apply_template_library_snapshot(
    db: &mut dyn ProjectDb,
    project: Project,
    snapshot: TemplateLibrarySnapshot,
) -> bool {
    let current = project.template_libraries(db).clone();
    let next = current.apply_active_snapshot(Some(snapshot));
    if project.template_libraries(db) == &next {
        return false;
    }

    project.set_template_libraries(db).to(next);
    true
}

#[derive(Serialize, Deserialize)]
struct CacheEnvelope {
    djls_version: String,
    response: TemplateLibrarySnapshot,
}

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
    let digest = hasher.finalize();
    let mut key = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut key, "{byte:02x}").expect("writing to String cannot fail");
    }
    key
}

fn cache_dir(
    root: &Utf8Path,
    interpreter: &Interpreter,
    django_settings_module: Option<&str>,
    pythonpath: &[String],
) -> Option<Utf8PathBuf> {
    let base = djls_conf::project_dirs()
        .and_then(|dirs| Utf8PathBuf::from_path_buf(dirs.cache_dir().to_path_buf()).ok())?;
    let key = cache_key(root, interpreter, django_settings_module, pythonpath);
    // Keep the legacy `inspector` directory for on-disk cache compatibility.
    Some(base.join("inspector").join(&key[..16]))
}

fn refresh_template_files(db: &mut dyn ProjectDb, project: Project) {
    let next = match project.template_dirs(db).as_known() {
        Some(search_dirs) => {
            let mut templates = Vec::new();
            let walk_options = WalkOptions::unrestricted();

            for dir in search_dirs {
                if !db.path_is_dir(dir) {
                    tracing::warn!("Template directory does not exist: {}", dir);
                    continue;
                }

                let mut dir_templates = Vec::new();
                let entries = match db.walk_entries(dir, &walk_options) {
                    Ok(entries) => entries,
                    Err(err) => {
                        tracing::warn!("Failed to walk template directory {}: {}", dir, err);
                        continue;
                    }
                };
                for entry in entries {
                    if entry.kind != WalkEntryKind::File {
                        continue;
                    }
                    let name = entry.relative.clean().to_string();
                    dir_templates.push((name, entry.path));
                }

                dir_templates.sort_by(|(a_name, a_path), (b_name, b_path)| {
                    a_name.cmp(b_name).then_with(|| a_path.cmp(b_path))
                });
                templates.extend(dir_templates);
            }

            ProjectTemplateFiles::from_ordered_paths(db, templates)
        }
        None => ProjectTemplateFiles::default(),
    };

    if project.template_files(db) != &next {
        project.set_template_files(db).to(next);
    }
}

fn refresh_python_modules(db: &mut dyn ProjectDb, project: Project) {
    // The LSP currently has no watched-file stream for dependency roots. Treat
    // an explicit refresh as the freshness boundary for module discovery and
    // currently discovered Python files.
    let roots: Vec<_> = project
        .search_paths(db)
        .iter()
        .filter_map(|search_path| db.files().root(db, search_path.path()))
        .collect();

    for root in roots {
        db.bump_file_root_revision(root);
    }

    let mut file_paths = Vec::new();
    file_paths.extend(
        model_modules(db, project)
            .iter()
            .map(|module| module.path().to_path_buf()),
    );

    file_paths.extend(
        templatetag_modules(db, project)
            .iter()
            .map(|module| module.path().to_path_buf()),
    );

    file_paths.sort();
    file_paths.dedup();

    for path in file_paths {
        let file = db.get_or_create_file(&path);
        db.bump_file_revision(file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn cache_envelope_round_trips() {
        let snapshot = TemplateLibrarySnapshot {
            symbols: Vec::new(),
            libraries: [("i18n".to_string(), "django.templatetags.i18n".to_string())].into(),
            builtins: vec!["django.template.defaulttags".to_string()],
        };
        let envelope = CacheEnvelope {
            djls_version: "test-version".to_string(),
            response: snapshot.clone(),
        };

        let json = serde_json::to_string(&envelope).unwrap();
        let decoded: CacheEnvelope = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.djls_version, "test-version");
        assert_eq!(decoded.response, snapshot);
    }
}

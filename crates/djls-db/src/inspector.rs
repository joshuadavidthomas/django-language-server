use djls_semantic::ProjectDb;
use salsa::Setter;

/// Populate template libraries from the filesystem cache, if available.
///
/// This is a fast, synchronous operation that loads a previously cached
/// template library snapshot from disk. Returns `true` if the cache was loaded
/// successfully (meaning we can defer the real backend query to the background).
pub fn load_inspector_cache(db: &mut dyn ProjectDb) -> bool {
    let Some(project) = db.project() else {
        return false;
    };

    let interpreter = project.interpreter(db).clone();
    let root = project.root(db).clone();
    let dsm = project.django_settings_module(db).clone();
    let pythonpath = project.pythonpath(db).clone();

    let Some(response) = djls_semantic::load_cached_template_library_snapshot(
        &root,
        &interpreter,
        dsm.as_deref(),
        &pythonpath,
    ) else {
        return false;
    };

    let current = project.template_libraries(db).clone();
    let next = current.apply_active_snapshot(Some(response));
    if project.template_libraries(db) != &next {
        project.set_template_libraries(db).to(next);
    }

    true
}

/// Query the Python inspector subprocess and update the project's template libraries.
pub(crate) fn query_inspector_template_libraries(db: &mut dyn ProjectDb) {
    let Some(project) = db.project() else {
        return;
    };

    let interpreter = project.interpreter(db).clone();
    let root = project.root(db).clone();
    let dsm = project.django_settings_module(db).clone();
    let pythonpath = project.pythonpath(db).clone();

    let response = djls_semantic::fetch_template_library_snapshot(db);

    if let Some(ref response) = response {
        djls_semantic::save_template_library_snapshot(
            &root,
            &interpreter,
            dsm.as_deref(),
            &pythonpath,
            response,
        );
    }

    let current = project.template_libraries(db).clone();
    let next = current.apply_active_snapshot(response);
    if project.template_libraries(db) != &next {
        project.set_template_libraries(db).to(next);
    }
}

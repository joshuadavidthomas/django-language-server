use djls_project::Db as ProjectDb;
use djls_project::TemplateLibrariesRequest;
use djls_project::TemplateLibrariesResponse;
use salsa::Setter;

/// Populate template libraries from the filesystem cache, if available.
///
/// This is a fast, synchronous operation that loads a previously cached
/// inspector response from disk. Returns `true` if the cache was loaded
/// successfully (meaning we can defer the real inspector query to the
/// background).
pub fn load_inspector_cache(db: &mut dyn ProjectDb) -> bool {
    let Some(project) = db.project() else {
        return false;
    };

    let interpreter = project.interpreter(db).clone();
    let root = project.root(db).clone();
    let dsm = project.django_settings_module(db).clone();
    let pythonpath = project.pythonpath(db).clone();

    let Some(response) = djls_project::load_cached_inspector_response(
        &root,
        &interpreter,
        dsm.as_deref(),
        &pythonpath,
    ) else {
        return false;
    };

    let current = project.template_libraries(db).clone();
    let next = current.apply_inspector(Some(response));
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

    let inspector = db.inspector();

    let interpreter = project.interpreter(db).clone();
    let root = project.root(db).clone();
    let dsm = project.django_settings_module(db).clone();
    let pythonpath = project.pythonpath(db).clone();
    let env_vars = project.env_vars(db).clone();

    let response = match inspector.query::<TemplateLibrariesRequest, TemplateLibrariesResponse>(
        &interpreter,
        &root,
        dsm.as_deref(),
        &pythonpath,
        &env_vars,
        &TemplateLibrariesRequest,
    ) {
        Ok(response) if response.ok => response.data,
        Ok(response) => {
            if let Some(ref error) = response.error {
                tracing::warn!("query_inspector: inspector failed: {}", error);
            } else {
                tracing::warn!("query_inspector: inspector returned an error with no details");
            }
            None
        }
        Err(e) => {
            tracing::error!("query_inspector: inspector query failed: {}", e);
            None
        }
    };

    if let Some(ref response) = response {
        djls_project::save_inspector_response(
            &root,
            &interpreter,
            dsm.as_deref(),
            &pythonpath,
            response,
        );
    }

    let current = project.template_libraries(db).clone();
    let next = current.apply_inspector(response);
    if project.template_libraries(db) != &next {
        project.set_template_libraries(db).to(next);
    }
}

use std::sync::Arc;

use djls_conf::DiagnosticSeverity;
use djls_conf::Settings;
use djls_db::DjangoDatabase;
use djls_semantic::Db as _;
use djls_source::InMemoryFileSystem;

#[test]
fn only_exactly_cased_django_suppresses_bundles() {
    use djls_project::Db as _;
    for (name, bundled) in [
        ("Django.py", true),
        ("Django/placeholder.py", true),
        ("django.py", false),
        ("django/placeholder.py", false),
    ] {
        let root = camino::Utf8Path::new("/project");
        let mut fs = InMemoryFileSystem::case_insensitive();
        fs.add_file(root.join(name), "# local module".into());
        let settings: Settings = serde_json::from_value(
            serde_json::json!({"venv_path": "/missing-venv", "django_version": "5.2"}),
        )
        .expect("settings");
        let mut db = DjangoDatabase::new(
            Arc::new(djls_project::BundledFileSystem::new(Arc::new(fs))),
            &settings,
            Some(root),
        );
        db.apply_project_settings(settings);
        djls_project::run_django_discovery(&mut db).expect("discovery");
        let resolved = djls_project::PythonSourceModule::resolve(
            &db,
            db.project().expect("project"),
            djls_project::PythonModuleName::parse("django.template.defaulttags").expect("module"),
        );
        if bundled {
            let module = resolved.expect("bundled module must resolve");
            assert!(
                module
                    .file()
                    .try_source(&db)
                    .expect("source")
                    .as_str()
                    .contains("def do_if("),
                "{name}"
            );
        } else {
            assert!(
                resolved.is_none(),
                "{name}: local Django must not be filled in"
            );
        }
    }
}

#[test]
fn diagnostics_configuration_is_owned_by_each_database_snapshot() {
    let settings: Settings = serde_json::from_value(serde_json::json!({
        "django_settings_module": "project.settings",
        "pythonpath": ["/project/vendor", "/project/apps"],
        "tagspecs": {
            "version": "0.6.0", "engine": "django",
            "libraries": [{"module": "app.templatetags.custom", "tags": [
                {"name": "panel", "type": "block", "end": {"name": "endpanel"}}
            ]}]
        },
        "diagnostics": {"severity": {"S": "warning", "S100": "off"}}
    }))
    .expect("settings should deserialize");
    let mut db = DjangoDatabase::new(Arc::new(InMemoryFileSystem::new()), &settings, None);
    let snapshot = db.clone();
    let original = db.diagnostics_config();
    assert_eq!(original, settings.diagnostics().clone());
    assert_eq!(original.get_severity("S100"), DiagnosticSeverity::Off);
    assert_eq!(original.get_severity("S101"), DiagnosticSeverity::Warning);

    let replacement: Settings = serde_json::from_value(serde_json::json!({
        "diagnostics": {"severity": {"S100": "hint"}}
    }))
    .expect("replacement settings should deserialize");
    db.apply_project_settings(replacement);
    assert_eq!(
        db.diagnostics_config().get_severity("S100"),
        DiagnosticSeverity::Hint
    );
    assert_eq!(
        db.diagnostics_config().get_severity("S101"),
        DiagnosticSeverity::Error
    );
    assert_eq!(snapshot.diagnostics_config(), original);
    assert_eq!(snapshot.settings(), settings);

    let mut detached = db.diagnostics_config();
    detached.set_severity("S100", DiagnosticSeverity::Error);
    assert_eq!(
        db.diagnostics_config().get_severity("S100"),
        DiagnosticSeverity::Hint
    );
}

fn bundled_database(
    root: &camino::Utf8Path,
    version: &str,
) -> Result<DjangoDatabase, Box<dyn std::error::Error>> {
    let settings: Settings = serde_json::from_value(serde_json::json!({
        "django_settings_module": "settings",
        "venv_path": root.join("missing-venv"),
        "django_version": version,
    }))?;
    let mut db = DjangoDatabase::new(
        Arc::new(djls_project::BundledFileSystem::new(Arc::new(
            djls_source::OsFileSystem::default(),
        ))),
        &settings,
        Some(root),
    );
    db.apply_project_settings(settings);
    djls_project::run_django_discovery(&mut db)?;
    Ok(db)
}

#[test]
fn bundled_versions_supply_standalone_tags_filters_and_loadable_libraries() {
    use djls_project::Db as _;
    use djls_project::ScopedTemplateLibraries;
    use djls_project::template_library_catalog;
    use djls_semantic::collect_template_diagnostics;
    use djls_source::path_to_file;

    let temp = tempfile::tempdir().expect("project directory");
    let root = camino::Utf8Path::from_path(temp.path()).expect("UTF-8 root");
    std::fs::write(root.join("valid.html"), "{% load i18n %}{% translate 'hello' %}{% for x in xs %}{{ x|default:'empty' }}{% empty %}Empty{% endfor %}{% widthratio x y 100 %}").expect("template");
    std::fs::write(root.join("invalid.html"), "{{ value|default }}").expect("template");
    std::fs::write(root.join("unloaded.html"), "{% translate 'hello' %}").expect("template");
    std::fs::write(root.join("invalid-tag.html"), "{% widthratio x y %}").expect("template");
    std::fs::write(root.join("humanize.html"), "{% load humanize %}").expect("template");
    std::fs::write(root.join("custom.html"), "{% load project_tags %}").expect("template");
    for version in ["5.2", "6.0", "6.1"] {
        let db = bundled_database(root, version).expect("bundled database");
        let project = db.project().expect("project");
        let inventory =
            ScopedTemplateLibraries::from_project_inventory(template_library_catalog(&db, project));
        let library = inventory
            .loadable_library_str("i18n")
            .found()
            .expect("i18n library");
        assert!(
            library
                .symbols()
                .iter()
                .any(|symbol| symbol.name() == "translate")
        );
        let valid = path_to_file(&db, &root.join("valid.html")).expect("valid file");
        let diagnostics = collect_template_diagnostics(&db, valid);
        assert!(
            !diagnostics.has_diagnostics(),
            "{version}: {:?}",
            diagnostics.validation_errors
        );
        for (name, code) in [
            ("invalid.html", "S115"),
            ("unloaded.html", "S109"),
            ("invalid-tag.html", "S117"),
        ] {
            let file = path_to_file(&db, &root.join(name)).expect("invalid file");
            let diagnostics = collect_template_diagnostics(&db, file);
            assert_eq!(
                diagnostics.validation_errors.len(),
                1,
                "{version}: {name}: {:?}",
                diagnostics.validation_errors
            );
            assert_eq!(diagnostics.validation_errors[0].code(), code);
        }
        for name in ["humanize.html", "custom.html"] {
            let file = path_to_file(&db, &root.join(name)).expect("open-evidence file");
            let diagnostics = collect_template_diagnostics(&db, file);
            assert!(
                !diagnostics
                    .validation_errors
                    .iter()
                    .any(|error| error.code() == "S120"),
                "{version}: {name}: {:?}",
                diagnostics.validation_errors
            );
        }
        let bundle = project
            .search_paths(&db)
            .iter()
            .last()
            .expect("bundle root")
            .path();
        let init = djls_source::Db::file_system(&db)
            .read_to_string(&bundle.join("django/__init__.py"))
            .expect("archive source");
        assert!(init.contains(&format!("VERSION = ({},", version.replace('.', ", "))));
    }
}

#[test]
fn bundled_selection_reloads_and_never_fills_in_installed_django() {
    use djls_project::Db as _;
    use djls_project::PythonModuleName;
    use djls_project::PythonSourceModule;

    let temp = tempfile::tempdir().expect("project directory");
    let root = camino::Utf8Path::from_path(temp.path()).expect("UTF-8 root");
    let mut db = bundled_database(root, "5.2").expect("bundled database");
    let project = db.project().expect("project");
    let old_paths = project.search_paths(&db).clone();
    let settings: Settings = serde_json::from_value(serde_json::json!({
        "django_settings_module": "settings",
        "venv_path": root.join("missing-venv"),
        "django_version": "6.0",
    }))
    .expect("replacement settings");
    db.apply_project_settings(settings);
    djls_project::run_django_discovery(&mut db).expect("reload");
    assert_ne!(&old_paths, project.search_paths(&db));
    let name = PythonModuleName::parse("django.template.defaulttags").expect("module name");
    let module = PythonSourceModule::resolve(&db, project, name.clone()).expect("bundled module");
    assert!(
        module
            .file()
            .try_source(&db)
            .expect("source")
            .as_str()
            .contains("partialdef")
    );

    std::fs::create_dir(root.join("django")).expect("installed package");
    std::fs::write(root.join("django/__init__.py"), "# Local Django fork\n").expect("source");
    djls_project::run_django_discovery(&mut db).expect("installed reload");
    assert_eq!(project.search_paths(&db).iter().count(), 1);
    assert!(PythonSourceModule::resolve(&db, project, name).is_none());
    std::fs::remove_dir_all(root.join("django")).expect("remove test installation");
    djls_project::run_django_discovery(&mut db).expect("fallback reload");
    assert_eq!(project.search_paths(&db).iter().count(), 2);

    let settings: Settings = serde_json::from_value(serde_json::json!({
        "django_settings_module": "settings",
        "venv_path": root.join("missing-venv"),
    }))
    .expect("settings without override");
    db.apply_project_settings(settings);
    djls_project::run_django_discovery(&mut db).expect("clear override");
    assert_eq!(project.django_version(&db), None);
    assert_eq!(&old_paths, project.search_paths(&db));
}

#[test]
fn bundled_admin_templates_follow_installed_apps_and_loaders() {
    use djls_project::Db as _;
    use djls_project::TemplateName;
    use djls_project::TemplateResolutionResult;
    use djls_project::template_resolution;

    let temp = tempfile::tempdir().expect("project directory");
    let root = camino::Utf8Path::from_path(temp.path()).expect("UTF-8 root");
    for (apps, app_dirs, available) in [
        ("[]", "True", false),
        ("['django.contrib.admin']", "False", false),
        ("['django.contrib.admin']", "True", true),
    ] {
        std::fs::write(root.join("settings.py"), format!(
            "INSTALLED_APPS = {apps}\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': {app_dirs}}}]\n"
        )).expect("settings source");
        let db = bundled_database(root, "6.1").expect("bundled database");
        let resolution = template_resolution(&db, db.project().expect("project"));
        let name = TemplateName::new(&db, "admin/base.html".to_string());
        match resolution.resolve(&db, name) {
            TemplateResolutionResult::Found(origin) => {
                assert!(available, "apps={apps}, app_dirs={app_dirs}");
                let file = origin.file(&db);
                assert!(
                    djls_source::Db::file_system(&db).is_file(file.path(&db)),
                    "template must exist in archive filesystem"
                );
                assert!(
                    file.try_source(&db)
                        .expect("template source")
                        .as_str()
                        .contains("{% block title %}")
                );
            }
            TemplateResolutionResult::DoesNotExist(_) => assert!(!available),
            TemplateResolutionResult::Inconclusive(_) => {
                panic!("explicit settings should be conclusive")
            }
        }
    }
}

#[test]
fn inferred_bundled_version_changes_on_project_reload() {
    use djls_project::Db as _;
    use djls_project::PythonModuleName;
    use djls_project::PythonSourceModule;

    let temp = tempfile::tempdir().expect("project directory");
    let root = camino::Utf8Path::from_path(temp.path()).expect("UTF-8 root");
    let settings: Settings = serde_json::from_value(serde_json::json!({
        "django_settings_module": "settings",
        "venv_path": root.join("missing-venv"),
    }))
    .expect("auto settings");
    let mut db = DjangoDatabase::new(
        Arc::new(djls_project::BundledFileSystem::new(Arc::new(
            djls_source::OsFileSystem::default(),
        ))),
        &settings,
        Some(root),
    );
    db.apply_project_settings(settings);
    let project = db.project().expect("project");
    for (metadata, expected) in [
        (None, "VERSION = (5, 2,"),
        (
            Some((
                "pyproject.toml",
                "[project]\ndependencies = ['Django>=6.0,<7']\n",
            )),
            "VERSION = (6, 0,",
        ),
        (
            Some((
                "uv.lock",
                "[[package]]\nname = 'project'\nsource = { virtual = '.' }\ndependencies = [{name = 'django'}]\n[[package]]\nname = 'django'\nversion = '6.1.0'\n",
            )),
            "VERSION = (6, 1,",
        ),
    ] {
        if let Some((name, content)) = metadata {
            std::fs::write(root.join(name), content).expect("dependency metadata");
        }
        djls_project::run_django_discovery(&mut db).expect("discovery");
        let module = PythonSourceModule::resolve(
            &db,
            project,
            PythonModuleName::parse("django").expect("module name"),
        )
        .expect("bundled Django");
        assert!(
            module
                .file()
                .try_source(&db)
                .expect("source")
                .as_str()
                .contains(expected)
        );
    }
}

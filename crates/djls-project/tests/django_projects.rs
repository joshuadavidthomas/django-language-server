use std::io;
use std::path::PathBuf;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_project::FileModuleCandidate;
use djls_project::FileModuleResolution;
use djls_project::Project;
use djls_project::PythonModuleName;
use djls_project::PythonSourceModule;
use djls_project::ScopedTemplateLibraries;
use djls_project::SearchPath;
use djls_project::compute_model_graph;
use djls_project::file_to_module;
use djls_project::file_to_module_resolution;
use djls_project::resolve_package_dirs;
use djls_project::template_directories;
use djls_project::template_library_catalog;
use djls_project::testing::compute_project_facts;
use djls_project::testing::django_settings;
use djls_project::testing::settings_module_file;
use djls_testing::OsTestDatabase;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

fn fixture_root(name: &str) -> TestResult<Utf8PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../djls-testing/fixtures/django-projects")
        .join(name)
        .canonicalize()?;
    Ok(Utf8PathBuf::from_path_buf(root).map_err(|path| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("fixture path should be UTF-8: {}", path.display()),
        )
    })?)
}

fn bootstrap_fixture(
    name: &str,
    overrides: Option<serde_json::Map<String, serde_json::Value>>,
) -> TestResult<(OsTestDatabase, Project, Utf8PathBuf)> {
    let root = fixture_root(name)?;
    let mut db = OsTestDatabase::with_disk_roots([root.clone()]);
    let settings = djls_conf::Settings::new(root.as_path(), overrides)?;
    let project = Project::bootstrap(&db, root.as_path(), &settings);
    db.set_project(project);
    Ok((db, project, root))
}

fn template_dirs(db: &OsTestDatabase, project: Project) -> Vec<Utf8PathBuf> {
    let directories = template_directories(db, project);
    assert!(!directories.settings_cases_may_omit_roots());
    directories
        .known_roots()
        .map(Utf8Path::to_path_buf)
        .collect()
}

#[test]
fn src_layout_discovers_nested_roots_settings_models_and_libraries() {
    let root = fixture_root("src-layout").expect("src-layout fixture root should resolve");
    let venv = root.join(".venv");
    let overrides = serde_json::Map::from_iter([("venv_path".into(), serde_json::json!(venv))]);
    let (db, project, root) = bootstrap_fixture("src-layout", Some(overrides))
        .expect("src-layout fixture should bootstrap");

    let site_packages = root.join(".venv/lib/python3.12/site-packages");
    let search_paths: Vec<_> = project.search_paths(&db).iter().cloned().collect();
    assert_eq!(
        search_paths,
        vec![
            SearchPath::FirstParty(root.join("src")),
            SearchPath::FirstParty(root.clone()),
            SearchPath::SitePackages(site_packages),
        ]
    );

    let discovery = compute_project_facts(&db, project);
    assert!(
        !discovery
            .file_paths()
            .contains(&root.join("src/blog/models.py")),
        "routine discovery should defer model sources until a Model query needs them"
    );

    let dirs = template_dirs(&db, project);
    assert!(dirs.contains(&root.join("src/blog/templates")));

    let models = compute_model_graph(&db, project);
    let post_modules: Vec<_> = models
        .models_named("Post")
        .map(|(id, _model)| id.module_name().as_str())
        .collect();
    assert_eq!(post_modules, vec!["blog.models"]);

    let models_module = file_to_module(&db, project, root.join("src/blog/models.py"))
        .expect("blog models file should map to a module");
    assert_eq!(models_module.name().as_str(), "blog.models");

    let libraries = template_library_catalog(&db, project);
    let blog_tags = ScopedTemplateLibraries::from_project_inventory(libraries)
        .loadable_library_str("blog_tags")
        .found()
        .expect("blog_tags should be installed");
    assert_eq!(blog_tags.module_name_str(), "blog.templatetags.blog_tags");
}

#[test]
fn editable_pth_discovers_editable_roots_libraries_and_shadowing() {
    let root = fixture_root("editable-pth").expect("editable-pth fixture root should resolve");
    let venv = root.join(".venv");
    let overrides = serde_json::Map::from_iter([("venv_path".into(), serde_json::json!(venv))]);
    let (db, project, root) = bootstrap_fixture("editable-pth", Some(overrides))
        .expect("editable-pth fixture should bootstrap");

    let site_packages = root.join(".venv/lib/python3.12/site-packages");
    let search_paths: Vec<_> = project.search_paths(&db).iter().cloned().collect();
    assert_eq!(
        search_paths,
        vec![
            SearchPath::FirstParty(root.clone()),
            SearchPath::SitePackages(site_packages),
            SearchPath::Editable(root.join("vendor")),
        ]
    );

    let dirs = template_dirs(&db, project);
    assert!(dirs.contains(&root.join("vendor/shoutbox/templates")));

    let libraries = template_library_catalog(&db, project);
    let shout_tags = ScopedTemplateLibraries::from_project_inventory(libraries)
        .loadable_library_str("shout_tags")
        .found()
        .expect("shout_tags should be installed");
    assert_eq!(
        shout_tags.module_name_str(),
        "shoutbox.templatetags.shout_tags"
    );

    let dupe_name =
        PythonModuleName::parse("dupe").expect("test Python module name should be valid");
    let dupe = PythonSourceModule::resolve(&db, project, dupe_name.clone())
        .expect("dupe should resolve to the first root");
    assert_eq!(dupe.path(), root.join("dupe.py").as_path());

    assert_eq!(dupe.search_path(), &SearchPath::FirstParty(root.clone()));

    let vendor_dupe = root.join("vendor/dupe.py");
    let vendor_module = file_to_module(&db, project, vendor_dupe.clone())
        .expect("vendored dupe should map through the first-party namespace");
    assert_eq!(vendor_module.name().as_str(), "vendor.dupe");
    assert_eq!(vendor_module.path(), vendor_dupe.as_path());

    assert_eq!(
        file_to_module_resolution(&db, project, vendor_dupe),
        &FileModuleResolution::Candidates {
            first: FileModuleCandidate::Resolved(vendor_module),
            rest: vec![FileModuleCandidate::Shadowed {
                root: SearchPath::Editable(root.join("vendor")),
                name: dupe_name,
                winner: dupe,
            }],
        }
    );
}

#[test]
fn namespace_apps_discovers_namespace_dirs_config_tails_and_libraries() {
    let root = fixture_root("namespace-apps").expect("namespace-apps fixture root should resolve");
    let venv = root.join(".venv");
    let overrides = serde_json::Map::from_iter([("venv_path".into(), serde_json::json!(venv))]);
    let (db, project, root) = bootstrap_fixture("namespace-apps", Some(overrides))
        .expect("namespace-apps fixture should bootstrap");

    let site_packages = root.join(".venv/lib/python3.12/site-packages");
    let search_paths: Vec<_> = project.search_paths(&db).iter().cloned().collect();
    assert_eq!(
        search_paths,
        vec![
            SearchPath::FirstParty(root.clone()),
            SearchPath::SitePackages(site_packages),
        ]
    );

    let dirs = template_dirs(&db, project);
    assert!(dirs.contains(&root.join("nsapp/templates")));
    assert!(dirs.contains(&root.join("checkout/templates")));
    assert!(dirs.contains(&root.join("weird/templates")));

    let libraries = template_library_catalog(&db, project);
    let ns_tags = ScopedTemplateLibraries::from_project_inventory(libraries)
        .loadable_library_str("ns_tags")
        .found()
        .expect("ns_tags should be installed from namespace app");
    assert_eq!(ns_tags.module_name_str(), "nsapp.templatetags.ns_tags");

    let nsapp_name =
        PythonModuleName::parse("nsapp").expect("test Python module name should be valid");
    let package_dirs = resolve_package_dirs(&db, project, nsapp_name.clone());
    assert_eq!(package_dirs.dirs, vec![root.join("nsapp")]);

    assert_eq!(PythonSourceModule::resolve(&db, project, nsapp_name), None);
}

#[test]
fn gh401_multisite_keeps_settings_environments_separate() {
    for (settings_module, settings_path, expected_apps, expected_dirs, libraries, absent_library) in [
        (
            "site1.settings.dev",
            "projects/site1/settings/dev.py",
            [
                "django.contrib.auth",
                "django.contrib.contenttypes",
                "clientname.app1",
                "clientname.app2",
            ],
            [
                "projects/site1/templates",
                "apps/clientname/app1/templates",
                "apps/clientname/app2/templates",
            ],
            [
                ("app1_tags", "clientname.app1.templatetags.app1_tags"),
                ("app2_tags", "clientname.app2.templatetags.app2_tags"),
            ],
            "app3_tags",
        ),
        (
            "site2.settings.dev",
            "projects/site2/settings/dev.py",
            [
                "django.contrib.auth",
                "django.contrib.contenttypes",
                "clientname.app2",
                "clientname.app3",
            ],
            [
                "projects/site2/templates",
                "apps/clientname/app2/templates",
                "apps/clientname/app3/templates",
            ],
            [
                ("app2_tags", "clientname.app2.templatetags.app2_tags"),
                ("app3_tags", "clientname.app3.templatetags.app3_tags"),
            ],
            "app1_tags",
        ),
    ] {
        let overrides = serde_json::Map::from_iter([(
            "django_settings_module".into(),
            serde_json::json!(settings_module),
        )]);
        let (db, project, root) = bootstrap_fixture("gh401-multisite", Some(overrides))
            .expect("GH-401 multi-site fixture should bootstrap");

        let settings_file = settings_module_file(&db, project)
            .expect("configured GH-401 settings module should resolve");
        assert_eq!(settings_file.path(&db), root.join(settings_path).as_path());

        let settings = serde_json::to_value(django_settings(&db, project))
            .expect("GH-401 settings should serialize");
        let apps = settings["installed_apps"]["cases"][0]["known"]["apps"]
            .as_array()
            .expect("GH-401 installed apps should be statically known")
            .iter()
            .map(|app| {
                app["value"]
                    .as_str()
                    .expect("GH-401 installed app should be a string")
            })
            .collect::<Vec<_>>();
        assert_eq!(apps, expected_apps);

        let dirs = template_directories(&db, project)
            .known_roots()
            .map(Utf8Path::to_path_buf)
            .collect::<Vec<_>>();
        for expected_dir in expected_dirs {
            assert!(
                dirs.contains(&root.join(expected_dir)),
                "{settings_module} should include template directory `{expected_dir}`, found {dirs:?}"
            );
        }

        let catalog = template_library_catalog(&db, project);
        let scoped = ScopedTemplateLibraries::from_project_inventory(catalog);
        for (load_name, module_name) in libraries {
            let library = scoped
                .loadable_library_str(load_name)
                .found()
                .unwrap_or_else(|| panic!("{load_name} should be loadable for {settings_module}"));
            assert_eq!(library.module_name_str(), module_name);
        }
        assert!(
            scoped
                .loadable_library_str(absent_library)
                .found()
                .is_none(),
            "{absent_library} should not be loadable for {settings_module}"
        );
    }
}

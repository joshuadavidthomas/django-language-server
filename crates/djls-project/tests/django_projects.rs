use std::path::PathBuf;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_project::CandidateKind;
use djls_project::CandidateLocation;
use djls_project::FileModuleDerivationStatus;
use djls_project::Project;
use djls_project::PythonModule;
use djls_project::PythonModuleName;
use djls_project::SearchPath;
use djls_project::UnresolvedReason;
use djls_project::compute_model_graph;
use djls_project::file_to_module;
use djls_project::file_to_module_detail;
use djls_project::resolve_module_detail;
use djls_project::resolve_package_dirs;
use djls_project::template_libraries;
use djls_project::template_resolution;
use djls_project::testing::compute_django_discovery;
use djls_testing::OsTestDatabase;

fn fixture_root(name: &str) -> Utf8PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../djls-testing/fixtures/django-projects")
        .join(name)
        .canonicalize()
        .unwrap_or_else(|err| panic!("failed to canonicalize fixture `{name}`: {err}"));
    Utf8PathBuf::from_path_buf(root).expect("fixture path should be UTF-8")
}

fn bootstrap_fixture(
    name: &str,
    overrides: Option<djls_conf::Settings>,
) -> (OsTestDatabase, Project, Utf8PathBuf) {
    let root = fixture_root(name);
    let mut db = OsTestDatabase::new();
    let settings = djls_conf::Settings::new(root.as_path(), overrides).unwrap();
    let project = Project::bootstrap(&db, root.as_path(), &settings);
    db.set_project(project);
    (db, project, root)
}

fn venv_override(venv: &Utf8Path) -> djls_conf::Settings {
    let escaped = venv.as_str().replace('\\', "\\\\").replace('"', "\\\"");
    toml::from_str::<djls_conf::Settings>(&format!("venv_path = \"{escaped}\""))
        .expect("venv override should deserialize")
}

fn template_dirs(db: &OsTestDatabase, project: Project) -> Vec<Utf8PathBuf> {
    template_resolution(db, project)
        .known_template_dirs(db)
        .expect("template dirs should be complete")
}

#[test]
fn src_layout_discovers_nested_roots_settings_models_and_libraries() {
    let root = fixture_root("src-layout");
    let venv = root.join(".venv");
    let (db, project, root) = bootstrap_fixture("src-layout", Some(venv_override(&venv)));

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

    let discovery = compute_django_discovery(&db, project);
    assert!(
        discovery
            .file_paths()
            .contains(&root.join("src/blog/models.py")),
        "Django discovery should include the blog model source"
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

    let libraries = template_libraries(&db, project);
    assert!(libraries.inventory_is_complete());
    let blog_tags = libraries
        .installed_library_str("blog_tags")
        .expect("blog_tags should be installed");
    assert_eq!(blog_tags.module_name_str(), "blog.templatetags.blog_tags");
}

#[test]
fn editable_pth_discovers_editable_roots_libraries_and_shadowing() {
    let root = fixture_root("editable-pth");
    let venv = root.join(".venv");
    let (db, project, root) = bootstrap_fixture("editable-pth", Some(venv_override(&venv)));

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

    let libraries = template_libraries(&db, project);
    assert!(libraries.inventory_is_complete());
    let shout_tags = libraries
        .installed_library_str("shout_tags")
        .expect("shout_tags should be installed");
    assert_eq!(
        shout_tags.module_name_str(),
        "shoutbox.templatetags.shout_tags"
    );

    let dupe_name = PythonModuleName::parse("dupe").unwrap();
    let dupe = PythonModule::resolve(&db, project, dupe_name.clone())
        .expect("dupe should resolve to the first root");
    assert_eq!(dupe.path(), root.join("dupe.py").as_path());

    let dupe_detail = resolve_module_detail(&db, project, dupe_name);
    assert_eq!(
        dupe_detail.selected_root,
        Some(SearchPath::FirstParty(root.clone()))
    );
    assert_eq!(
        dupe_detail.candidates,
        vec![
            CandidateLocation {
                root: SearchPath::FirstParty(root.clone()),
                path: root.join("dupe.py"),
                kind: CandidateKind::FileModule,
            },
            CandidateLocation {
                root: SearchPath::Editable(root.join("vendor")),
                path: root.join("vendor/dupe.py"),
                kind: CandidateKind::FileModule,
            },
        ]
    );

    let vendor_dupe = root.join("vendor/dupe.py");
    let vendor_module = file_to_module(&db, project, vendor_dupe.clone())
        .expect("vendored dupe should map through the first-party namespace");
    assert_eq!(vendor_module.name().as_str(), "vendor.dupe");
    assert_eq!(vendor_module.path(), vendor_dupe.as_path());

    let vendor_detail = file_to_module_detail(&db, project, vendor_dupe.clone());
    let selected = vendor_detail
        .selected_module
        .expect("vendored dupe should be selected through the first-party namespace");
    assert_eq!(selected.name().as_str(), "vendor.dupe");
    assert_eq!(selected.path(), vendor_dupe.as_path());
    assert_eq!(vendor_detail.unresolved_reason, None);
    assert!(vendor_detail.derivations.iter().any(|derivation| {
        derivation.root == SearchPath::Editable(root.join("vendor"))
            && derivation.name.as_str() == "dupe"
            && derivation.resolved_path == Some(root.join("dupe.py"))
            && derivation.status == FileModuleDerivationStatus::Shadowed
    }));
}

#[test]
fn namespace_apps_discovers_namespace_dirs_config_tails_and_libraries() {
    let root = fixture_root("namespace-apps");
    let venv = root.join(".venv");
    let (db, project, root) = bootstrap_fixture("namespace-apps", Some(venv_override(&venv)));

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

    let libraries = template_libraries(&db, project);
    assert!(libraries.inventory_is_complete());
    let ns_tags = libraries
        .installed_library_str("ns_tags")
        .expect("ns_tags should be installed from namespace app");
    assert_eq!(ns_tags.module_name_str(), "nsapp.templatetags.ns_tags");

    let nsapp_name = PythonModuleName::parse("nsapp").unwrap();
    let package_dirs = resolve_package_dirs(&db, project, nsapp_name.clone());
    assert_eq!(package_dirs.dirs, vec![root.join("nsapp")]);

    assert_eq!(
        PythonModule::resolve(&db, project, nsapp_name.clone()),
        None
    );
    let detail = resolve_module_detail(&db, project, nsapp_name);
    assert_eq!(detail.selected_root, None);
    assert_eq!(
        detail.unresolved_reason,
        Some(UnresolvedReason::NamespaceOnly)
    );
    assert_eq!(
        detail.candidates,
        vec![CandidateLocation {
            root: SearchPath::FirstParty(root.clone()),
            path: root.join("nsapp"),
            kind: CandidateKind::NamespacePortion,
        }]
    );
}

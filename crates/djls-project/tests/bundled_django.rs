//! Extraction snapshots for the bundled Django archives.
//!
//! The corpus checks Django out as project code, so canonical-identity summaries such as
//! `token_kwargs` never apply to it there. These snapshots mount the bundled archives as
//! site-packages, as production does, and record what DJLS extracts from every builtin and
//! loadable library Django ships.

use std::fmt::Write as _;

use djls_conf::DjangoVersion;
use djls_project::Db as _;
use djls_project::ScopedTemplateLibraries;
use djls_project::template_library_catalog;
use djls_testing::bundled_django_database;
use djls_testing::extract_bundle;
use djls_testing::sorted_snapshot;

#[test]
fn bundled_django_extraction() {
    for version in [
        DjangoVersion::Django52,
        DjangoVersion::Django60,
        DjangoVersion::Django61,
    ] {
        let db = bundled_django_database(version).expect("bundled Django project should install");
        let project = db.project().expect("bundled Django project");
        let catalog = template_library_catalog(&db, project);
        let mut libraries = ScopedTemplateLibraries::from_project_inventory(catalog)
            .resolved_libraries()
            .into_iter()
            .filter(|library| library.module_name_str().starts_with("django."))
            .collect::<Vec<_>>();
        libraries.sort_by_key(|library| library.module_name_str().to_string());

        let mut index = String::new();
        for library in &libraries {
            let load_name = library
                .load_name()
                .map_or("(builtin)", |name| name.as_str());
            writeln!(index, "{load_name:<16} {}", library.module_name_str())
                .expect("write to string");
        }
        insta::assert_snapshot!(format!("{}__libraries", version.as_str()), index);

        for library in libraries {
            let file = library
                .source_file()
                .expect("bundled Django library should have source");
            let bundle = extract_bundle(&db, file, library.module_name().clone());
            insta::assert_yaml_snapshot!(
                format!("{}__{}", version.as_str(), library.module_name_str()),
                sorted_snapshot(&bundle).expect("extraction snapshot should serialize")
            );
        }
    }
}

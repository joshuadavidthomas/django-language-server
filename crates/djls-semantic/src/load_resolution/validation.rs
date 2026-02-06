use djls_templates::Node;
use djls_templates::NodeList;
use djls_templates::tokens::TagDelimiter;
use salsa::Accumulator;

use crate::Db;
use crate::ValidationError;
use crate::ValidationErrorAccumulator;

use super::compute_loaded_libraries;
use super::symbols::AvailableSymbols;
use super::symbols::TagAvailability;

/// Validate tag scoping for all tags in a template.
///
/// Checks each tag against the set of tags available at its position
/// (builtins + tags from loaded libraries), producing:
///
/// - **S108** (`UnknownTag`): tag is not known at all (not in inspector inventory)
/// - **S109** (`UnloadedTag`): tag is known but its library isn't loaded
/// - **S110** (`AmbiguousUnloadedTag`): tag is known but defined in multiple unloaded libraries
///
/// **Guards:**
/// - If the inspector inventory is `None`, all scoping diagnostics are suppressed.
/// - Structural tags (openers, closers, intermediates with `TagSpec`s) are skipped.
/// - The `load` tag itself is skipped (it's a builtin that defines scoping).
pub fn validate_tag_scoping(db: &dyn Db, nodelist: NodeList<'_>) {
    let Some(inventory) = db.inspector_inventory() else {
        return;
    };

    let tag_specs = db.tag_specs();
    let loaded_libraries = compute_loaded_libraries(db, nodelist);

    for node in nodelist.nodelist(db) {
        let Node::Tag { name, span, .. } = node else {
            continue;
        };

        // Skip the "load" tag itself — it defines scoping, not a user-visible tag
        if name == "load" {
            continue;
        }

        // Skip closers and intermediates — their availability is determined
        // by their opener tag, not by load scoping.
        if is_closer_or_intermediate(name, &tag_specs) {
            continue;
        }

        let marker_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);
        let symbols = AvailableSymbols::at_position(&loaded_libraries, &inventory, span.start());

        match symbols.check(name) {
            TagAvailability::Available => {}
            TagAvailability::Unknown => {
                ValidationErrorAccumulator(ValidationError::UnknownTag {
                    tag: name.clone(),
                    span: marker_span,
                })
                .accumulate(db);
            }
            TagAvailability::Unloaded { library } => {
                ValidationErrorAccumulator(ValidationError::UnloadedTag {
                    tag: name.clone(),
                    library,
                    span: marker_span,
                })
                .accumulate(db);
            }
            TagAvailability::AmbiguousUnloaded { libraries } => {
                ValidationErrorAccumulator(ValidationError::AmbiguousUnloadedTag {
                    tag: name.clone(),
                    libraries,
                    span: marker_span,
                })
                .accumulate(db);
            }
        }
    }
}

/// Check whether a tag is a closer or intermediate — these are part of block
/// structure and their availability is determined by their opener tag, not by
/// load scoping. For example, `{% endif %}` and `{% else %}` should never
/// produce S108/S109/S110.
///
/// Openers and standalone tags are NOT excluded — they need scoping checks
/// even if they have a `TagSpec` (e.g., `{% trans %}` has a spec for argument
/// validation but still requires `{% load i18n %}`).
fn is_closer_or_intermediate(name: &str, tag_specs: &crate::TagSpecs) -> bool {
    tag_specs.get_end_spec_for_closer(name).is_some()
        || tag_specs.get_intermediate_spec(name).is_some()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::Mutex;

    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_project::TemplateTags;
    use djls_source::Db as SourceDb;
    use djls_source::File;
    use djls_templates::parse_template;
    use djls_workspace::FileSystem;
    use djls_workspace::InMemoryFileSystem;

    use crate::blocks::TagIndex;
    use crate::templatetags::django_builtin_specs;
    use crate::validate_nodelist;
    use crate::ValidationError;
    use crate::ValidationErrorAccumulator;
    use crate::TagSpecs;

    #[salsa::db]
    #[derive(Clone)]
    struct TestDatabase {
        storage: salsa::Storage<Self>,
        fs: Arc<Mutex<InMemoryFileSystem>>,
        inventory: Option<TemplateTags>,
    }

    impl TestDatabase {
        fn new() -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
                inventory: None,
            }
        }

        fn with_inventory(inventory: TemplateTags) -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
                inventory: Some(inventory),
            }
        }

        fn add_file(&self, path: &str, content: &str) {
            self.fs
                .lock()
                .unwrap()
                .add_file(path.into(), content.to_string());
        }
    }

    #[salsa::db]
    impl salsa::Database for TestDatabase {}

    #[salsa::db]
    impl djls_source::Db for TestDatabase {
        fn create_file(&self, path: &Utf8Path) -> File {
            File::new(self, path.to_owned(), 0)
        }

        fn get_file(&self, _path: &Utf8Path) -> Option<File> {
            None
        }

        fn read_file(&self, path: &Utf8Path) -> std::io::Result<String> {
            self.fs.lock().unwrap().read_to_string(path)
        }
    }

    #[salsa::db]
    impl djls_templates::Db for TestDatabase {}

    #[salsa::db]
    impl crate::Db for TestDatabase {
        fn tag_specs(&self) -> TagSpecs {
            django_builtin_specs()
        }

        fn tag_index(&self) -> TagIndex<'_> {
            TagIndex::from_specs(self)
        }

        fn template_dirs(&self) -> Option<Vec<Utf8PathBuf>> {
            None
        }

        fn diagnostics_config(&self) -> djls_conf::DiagnosticsConfig {
            djls_conf::DiagnosticsConfig::default()
        }

        fn inspector_inventory(&self) -> Option<TemplateTags> {
            self.inventory.clone()
        }
    }

    fn builtin_tag_json(name: &str, module: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "provenance": {"builtin": {"module": module}},
            "defining_module": module,
            "doc": null,
        })
    }

    fn library_tag_json(name: &str, load_name: &str, module: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "provenance": {"library": {"load_name": load_name, "module": module}},
            "defining_module": module,
            "doc": null,
        })
    }

    fn make_inventory(
        tags: &[serde_json::Value],
        libraries: &HashMap<String, String>,
        builtins: &[String],
    ) -> TemplateTags {
        let payload = serde_json::json!({
            "tags": tags,
            "libraries": libraries,
            "builtins": builtins,
        });
        serde_json::from_value(payload).unwrap()
    }

    fn test_inventory() -> TemplateTags {
        let tags = vec![
            builtin_tag_json("if", "django.template.defaulttags"),
            builtin_tag_json("for", "django.template.defaulttags"),
            builtin_tag_json("block", "django.template.loader_tags"),
            builtin_tag_json("csrf_token", "django.template.defaulttags"),
            library_tag_json("trans", "i18n", "django.templatetags.i18n"),
            library_tag_json("blocktrans", "i18n", "django.templatetags.i18n"),
            library_tag_json("static", "static", "django.templatetags.static"),
        ];

        let mut libraries = HashMap::new();
        libraries.insert(
            "i18n".to_string(),
            "django.templatetags.i18n".to_string(),
        );
        libraries.insert(
            "static".to_string(),
            "django.templatetags.static".to_string(),
        );

        let builtins = vec![
            "django.template.defaulttags".to_string(),
            "django.template.loader_tags".to_string(),
        ];

        make_inventory(&tags, &libraries, &builtins)
    }

    fn collect_scoping_errors(db: &TestDatabase, source: &str) -> Vec<ValidationError> {
        let path = "test.html";
        db.add_file(path, source);
        let file = db.create_file(Utf8Path::new(path));
        let nodelist = parse_template(db, file).expect("should parse");
        validate_nodelist(db, nodelist);

        validate_nodelist::accumulated::<ValidationErrorAccumulator>(db, nodelist)
            .into_iter()
            .map(|acc| acc.0.clone())
            .filter(|err| {
                matches!(
                    err,
                    ValidationError::UnknownTag { .. }
                        | ValidationError::UnloadedTag { .. }
                        | ValidationError::AmbiguousUnloadedTag { .. }
                )
            })
            .collect()
    }

    #[test]
    fn unknown_tag_produces_s108() {
        let db = TestDatabase::with_inventory(test_inventory());
        let errors = collect_scoping_errors(&db, "{% xyz %}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], ValidationError::UnknownTag { tag, .. } if tag == "xyz"),
            "Expected UnknownTag for 'xyz', got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn unloaded_library_tag_produces_s109() {
        let db = TestDatabase::with_inventory(test_inventory());
        let errors = collect_scoping_errors(&db, "{% trans 'hello' %}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                &errors[0],
                ValidationError::UnloadedTag { tag, library, .. }
                    if tag == "trans" && library == "i18n"
            ),
            "Expected UnloadedTag for 'trans' requiring 'i18n', got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn tag_in_multiple_libraries_produces_s110() {
        let tags = vec![
            library_tag_json("shared", "lib_a", "app.templatetags.lib_a"),
            library_tag_json("shared", "lib_b", "app.templatetags.lib_b"),
        ];
        let mut libraries = HashMap::new();
        libraries.insert("lib_a".to_string(), "app.templatetags.lib_a".to_string());
        libraries.insert("lib_b".to_string(), "app.templatetags.lib_b".to_string());
        let inventory = make_inventory(&tags, &libraries, &[]);

        let db = TestDatabase::with_inventory(inventory);
        let errors = collect_scoping_errors(&db, "{% shared %}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                &errors[0],
                ValidationError::AmbiguousUnloadedTag { tag, libraries, .. }
                    if tag == "shared" && libraries == &["lib_a", "lib_b"]
            ),
            "Expected AmbiguousUnloadedTag for 'shared', got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn inspector_unavailable_no_scoping_diagnostics() {
        // No inventory set — inspector unavailable
        let db = TestDatabase::new();
        let errors = collect_scoping_errors(&db, "{% xyz %}{% trans 'hello' %}");

        assert!(
            errors.is_empty(),
            "No scoping diagnostics when inspector unavailable, got: {errors:?}"
        );
    }

    #[test]
    fn structural_tags_skip_scoping_checks() {
        let db = TestDatabase::with_inventory(test_inventory());
        // endif, else, elif are structural — they shouldn't produce S108
        let errors = collect_scoping_errors(
            &db,
            "{% if True %}{% elif False %}{% else %}{% endif %}",
        );

        assert!(
            errors.is_empty(),
            "Structural tags should not produce scoping errors, got: {errors:?}"
        );
    }

    #[test]
    fn loaded_library_tag_no_error() {
        let db = TestDatabase::with_inventory(test_inventory());
        let errors = collect_scoping_errors(&db, "{% load i18n %}\n{% trans 'hello' %}");

        assert!(
            errors.is_empty(),
            "Loaded library tag should not produce error, got: {errors:?}"
        );
    }

    #[test]
    fn tag_before_load_produces_error() {
        let db = TestDatabase::with_inventory(test_inventory());
        let errors = collect_scoping_errors(&db, "{% trans 'hello' %}\n{% load i18n %}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], ValidationError::UnloadedTag { tag, library, .. }
                if tag == "trans" && library == "i18n"),
            "Tag before load should produce UnloadedTag, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn selective_import_makes_only_imported_symbol_available() {
        let db = TestDatabase::with_inventory(test_inventory());
        let source = "{% load trans from i18n %}\n{% trans 'hello' %}\n{% blocktrans %}{% endblocktrans %}";
        let errors = collect_scoping_errors(&db, source);

        // trans should be available, blocktrans should NOT (only selectively imported trans)
        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], ValidationError::UnloadedTag { tag, library, .. }
                if tag == "blocktrans" && library == "i18n"),
            "Selectively-unimported tag should produce UnloadedTag, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn builtin_tag_always_available() {
        let db = TestDatabase::with_inventory(test_inventory());
        // csrf_token is a builtin — should be available without any load
        let errors = collect_scoping_errors(&db, "{% csrf_token %}");

        assert!(
            errors.is_empty(),
            "Builtin tags should always be available, got: {errors:?}"
        );
    }

    #[test]
    fn load_tag_itself_not_flagged() {
        let db = TestDatabase::with_inventory(test_inventory());
        let errors = collect_scoping_errors(&db, "{% load i18n %}");

        assert!(
            errors.is_empty(),
            "Load tag itself should not be flagged, got: {errors:?}"
        );
    }
}

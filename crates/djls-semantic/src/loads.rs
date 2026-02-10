mod load;
mod symbols;
mod validation;

use djls_templates::Node;
use djls_templates::NodeList;
pub use load::parse_load_bits;
pub use load::LoadKind;
pub use load::LoadStatement;
pub use load::LoadedLibraries;
pub use symbols::AvailableSymbols;
pub use symbols::FilterAvailability;
pub use symbols::TagAvailability;
pub use validation::validate_filter_scoping;
pub use validation::validate_load_libraries;
pub use validation::validate_tag_scoping;

use crate::db::Db;

/// Compute the [`LoadedLibraries`] for a parsed template's node list.
///
/// Iterates all nodes, identifies `{% load %}` tags, parses each into a
/// [`LoadStatement`], and returns an ordered [`LoadedLibraries`] collection
/// that supports position-aware availability queries.
///
/// Cached by Salsa — re-computes only when the underlying [`NodeList`] changes.
#[salsa::tracked]
pub fn compute_loaded_libraries(db: &dyn Db, nodelist: NodeList<'_>) -> LoadedLibraries {
    let statements: Vec<LoadStatement> = nodelist
        .nodelist(db)
        .iter()
        .filter_map(|node| match node {
            Node::Tag {
                name, bits, span, ..
            } if name == "load" => {
                let kind = parse_load_bits(bits)?;
                Some(LoadStatement::new(*span, kind))
            }
            _ => None,
        })
        .collect();

    LoadedLibraries::new(statements)
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use djls_templates::parse_template;

    use super::*;
    use crate::testing::TestDatabase;

    fn parse_and_compute(db: &TestDatabase, source: &str) -> LoadedLibraries {
        let path = "test.html";
        db.add_file(path, source);
        let file = db.create_file(Utf8Path::new(path));
        let nodelist = parse_template(db, file).expect("should parse");
        compute_loaded_libraries(db, nodelist)
    }

    #[test]
    fn empty_template_no_loads() {
        let db = TestDatabase::new();
        let loaded = parse_and_compute(&db, "<p>Hello</p>");
        assert!(loaded.is_empty());
        assert!(loaded.statements().is_empty());
    }

    #[test]
    fn single_full_load() {
        let db = TestDatabase::new();
        let loaded = parse_and_compute(&db, "{% load i18n %}");
        assert_eq!(loaded.statements().len(), 1);
        assert_eq!(
            *loaded.statements()[0].kind(),
            LoadKind::FullLoad {
                libraries: vec!["i18n".into()]
            }
        );
    }

    #[test]
    fn multiple_full_loads() {
        let db = TestDatabase::new();
        let loaded = parse_and_compute(&db, "{% load i18n %}\n{% load static %}");
        assert_eq!(loaded.statements().len(), 2);
        assert_eq!(
            *loaded.statements()[0].kind(),
            LoadKind::FullLoad {
                libraries: vec!["i18n".into()]
            }
        );
        assert_eq!(
            *loaded.statements()[1].kind(),
            LoadKind::FullLoad {
                libraries: vec!["static".into()]
            }
        );
    }

    #[test]
    fn multi_library_load() {
        let db = TestDatabase::new();
        let loaded = parse_and_compute(&db, "{% load i18n static %}");
        assert_eq!(loaded.statements().len(), 1);
        assert_eq!(
            *loaded.statements()[0].kind(),
            LoadKind::FullLoad {
                libraries: vec!["i18n".into(), "static".into()]
            }
        );
    }

    #[test]
    fn selective_import() {
        let db = TestDatabase::new();
        let loaded = parse_and_compute(&db, "{% load trans from i18n %}");
        assert_eq!(loaded.statements().len(), 1);
        assert_eq!(
            *loaded.statements()[0].kind(),
            LoadKind::SelectiveImport {
                symbols: vec!["trans".into()],
                library: "i18n".into(),
            }
        );
    }

    #[test]
    fn loads_among_other_tags() {
        let db = TestDatabase::new();
        let source = r#"{% load i18n %}
<h1>{% trans "Hello" %}</h1>
{% load static %}
<link href="{% static 'style.css' %}">"#;
        let loaded = parse_and_compute(&db, source);
        assert_eq!(loaded.statements().len(), 2);
        assert_eq!(
            *loaded.statements()[0].kind(),
            LoadKind::FullLoad {
                libraries: vec!["i18n".into()]
            }
        );
        assert_eq!(
            *loaded.statements()[1].kind(),
            LoadKind::FullLoad {
                libraries: vec!["static".into()]
            }
        );
    }

    #[test]
    fn position_query_after_load() {
        let db = TestDatabase::new();
        // "{% load i18n %}" is 16 bytes. Span for inner content "load i18n" starts at 3.
        let loaded = parse_and_compute(&db, "{% load i18n %}{% trans 'hi' %}");
        // After the first load tag, i18n should be available
        let state = loaded.available_at(100);
        assert!(state.is_fully_loaded("i18n"));
    }

    #[test]
    fn position_query_before_load() {
        let db = TestDatabase::new();
        // Put text before the load
        let loaded = parse_and_compute(&db, "some text {% load i18n %}");
        // At position 0 (before the load), i18n should NOT be available
        let state = loaded.available_at(0);
        assert!(!state.is_fully_loaded("i18n"));
    }

    #[test]
    fn selective_then_full_via_template() {
        let db = TestDatabase::new();
        let source = "{% load trans from i18n %}\n{% load i18n %}";
        let loaded = parse_and_compute(&db, source);
        assert_eq!(loaded.statements().len(), 2);

        // After both loads, i18n should be fully loaded
        let state = loaded.available_at(200);
        assert!(state.is_fully_loaded("i18n"));
        assert!(state.selective_imports().get("i18n").is_none());
    }

    #[test]
    fn malformed_load_ignored() {
        let db = TestDatabase::new();
        // "{% load %}" with no args should be parsed as a tag but parse_load_bits returns None
        let loaded = parse_and_compute(&db, "{% load %}");
        assert!(loaded.is_empty());
    }

    #[test]
    fn non_load_tags_ignored() {
        let db = TestDatabase::new();
        let source = "{% if condition %}{% endif %}{% block header %}{% endblock %}";
        let loaded = parse_and_compute(&db, source);
        assert!(loaded.is_empty());
    }
}

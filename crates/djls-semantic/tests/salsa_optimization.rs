// Test infrastructure for Salsa optimization validation

use djls_semantic::{Db as SemanticDb, SemanticIndex, TagSpecs, TagIndex};
use djls_templates::Db as TemplatesDb;
use djls_source::{Db as SourceDb, File};
use camino::{Utf8Path, Utf8PathBuf};
use dashmap::DashMap;
use std::io;

/// Test database for Salsa optimization tests
#[salsa::db]
pub struct TestDatabase {
    storage: salsa::Storage<Self>,
    sources: DashMap<Utf8PathBuf, String>,
}

impl TestDatabase {
    pub fn new() -> Self {
        Self {
            storage: Default::default(),
            sources: DashMap::new(),
        }
    }
    
    /// Create a file with template content
    pub fn file_with_contents(&mut self, path: impl Into<Utf8PathBuf>, contents: &str) -> File {
        let path = path.into();
        self.sources.insert(path.clone(), contents.to_string());
        File::new(self, path, 0)
    }
    
    /// Parse a template string
    pub fn parse_template(&mut self, content: &str) -> Option<djls_templates::NodeList> {
        let file = self.file_with_contents("test.html", content);
        djls_templates::parse_template(self, file)
    }
    
    /// Create a semantic index from template content
    pub fn semantic_index(&mut self, content: &str) -> Option<SemanticIndex> {
        let nodelist = self.parse_template(content)?;
        Some(SemanticIndex::new(self, nodelist))
    }
}

#[salsa::db]
impl salsa::Database for TestDatabase {}

#[salsa::db]
impl SourceDb for TestDatabase {
    fn read_file_source(&self, path: &Utf8Path) -> io::Result<String> {
        Ok(self
            .sources
            .get(path)
            .map(|entry| entry.value().clone())
            .unwrap_or_default())
    }
}

#[salsa::db]
impl TemplatesDb for TestDatabase {}

#[salsa::db]
impl SemanticDb for TestDatabase {
    fn tag_specs(&self) -> TagSpecs {
        djls_semantic::django_builtin_specs()
    }

    fn tag_index(&self) -> TagIndex {
        let specs = self.tag_specs();
        TagIndex::from_specs(&specs)
    }
}

/// Helper to measure cache invalidation
pub fn count_salsa_events(db: &TestDatabase) -> usize {
    // This will be implemented when we add Salsa event tracking
    // For now, return 0 as placeholder
    0
}

/// Helper to measure memory usage
pub fn measure_memory_usage() -> usize {
    // This will be implemented with memory profiling
    // For now, return 0 as placeholder
    0
}

#[cfg(test)]
mod infrastructure_tests {
    use super::*;
    
    #[test]
    fn test_database_creation() {
        let mut db = TestDatabase::new();
        let file = db.file_with_contents("test.html", "Hello");
        assert_eq!(file.path(&db).as_str(), "test.html");
    }
    
    #[test]
    fn test_template_parsing() {
        let mut db = TestDatabase::new();
        let nodelist = db.parse_template("{% block content %}Hello{% endblock %}");
        assert!(nodelist.is_some());
    }
    
    #[test]
    fn test_semantic_index_creation() {
        let mut db = TestDatabase::new();
        let index = db.semantic_index("{% block content %}{{ user.name }}{% endblock %}");
        assert!(index.is_some());
    }
}
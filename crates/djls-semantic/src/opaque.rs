use djls_source::Span;
use djls_templates::NodeList;

use crate::blocks::build_block_tree;
use crate::blocks::BlockId;
use crate::blocks::BlockNode;
use crate::blocks::Blocks;
use crate::blocks::BranchKind;
use crate::blocks::Region;
use crate::Db;
use crate::TagSpecs;

/// Sorted, non-overlapping byte-offset spans where validation should be skipped.
///
/// Represents the interior content of opaque blocks like `{% verbatim %}` and
/// `{% comment %}`, where the template engine treats the content as raw text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct OpaqueRegions {
    spans: Vec<Span>,
}

impl OpaqueRegions {
    #[must_use]
    pub fn new(mut spans: Vec<Span>) -> Self {
        spans.sort_by_key(|s| s.start());
        Self { spans }
    }

    /// Returns `true` if the given byte position falls inside an opaque region.
    #[must_use]
    pub fn is_opaque(&self, position: u32) -> bool {
        self.spans
            .binary_search_by(|span| {
                if position < span.start() {
                    std::cmp::Ordering::Greater
                } else if position >= span.end() {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .is_ok()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }
}

/// Compute opaque regions for a template by scanning for opaque block tags.
///
/// Walks the block tree looking for tags whose `TagSpec` has `opaque: true`
/// (e.g., `{% verbatim %}`, `{% comment %}`). The content between the opener
/// and closer of such blocks is recorded as an opaque region.
pub fn compute_opaque_regions(db: &dyn Db, nodelist: NodeList<'_>) -> OpaqueRegions {
    let tag_specs = db.tag_specs();
    let block_tree = build_block_tree(db, nodelist);
    let blocks = block_tree.blocks(db);
    let mut spans = Vec::new();

    collect_opaque_spans_from_roots(block_tree.roots(db), blocks, &tag_specs, &mut spans);

    OpaqueRegions::new(spans)
}

fn collect_opaque_spans_from_roots(
    roots: &[BlockId],
    blocks: &Blocks,
    tag_specs: &TagSpecs,
    spans: &mut Vec<Span>,
) {
    for &root_id in roots {
        let region = blocks.get(root_id.index());
        collect_opaque_spans_from_region(region, blocks, tag_specs, spans);
    }
}

fn collect_opaque_spans_from_region(
    region: &Region,
    blocks: &Blocks,
    tag_specs: &TagSpecs,
    spans: &mut Vec<Span>,
) {
    for node in region.nodes() {
        match node {
            BlockNode::Branch {
                tag,
                body,
                kind: BranchKind::Opener,
                ..
            } => {
                // Nested block opener — check if it's opaque and recurse
                if let Some(spec) = tag_specs.get(tag) {
                    if spec.opaque {
                        let body_region = blocks.get(body.index());
                        collect_all_segment_spans(body_region, blocks, spans);
                    }
                }
                let body_region = blocks.get(body.index());
                collect_opaque_spans_from_region(body_region, blocks, tag_specs, spans);
            }
            BlockNode::Branch {
                tag,
                body,
                kind: BranchKind::Segment,
                ..
            } => {
                // Segment — if the tag is opaque, record the segment body as opaque
                if let Some(spec) = tag_specs.get(tag) {
                    if spec.opaque {
                        let body_region = blocks.get(body.index());
                        spans.push(*body_region.span());
                    }
                }
                // Recurse into segment body for nested blocks
                let body_region = blocks.get(body.index());
                collect_opaque_spans_from_region(body_region, blocks, tag_specs, spans);
            }
            BlockNode::Leaf { .. } => {}
        }
    }
}

/// Collect spans from all segments in a container (for nested opaque blocks).
fn collect_all_segment_spans(region: &Region, blocks: &Blocks, spans: &mut Vec<Span>) {
    for node in region.nodes() {
        if let BlockNode::Branch {
            body,
            kind: BranchKind::Segment,
            ..
        } = node
        {
            let body_region = blocks.get(body.index());
            spans.push(*body_region.span());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_source::Db as SourceDb;
    use djls_source::File;
    use djls_templates::parse_template;
    use djls_workspace::FileSystem;
    use djls_workspace::InMemoryFileSystem;

    use super::*;
    use crate::blocks::TagIndex;
    use crate::templatetags::test_tag_specs;

    #[salsa::db]
    #[derive(Clone)]
    struct TestDatabase {
        storage: salsa::Storage<Self>,
        fs: Arc<Mutex<InMemoryFileSystem>>,
    }

    impl TestDatabase {
        fn new() -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
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
            test_tag_specs()
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

        fn template_libraries(&self) -> djls_project::TemplateLibraries {
            djls_project::TemplateLibraries::default()
        }

        fn filter_arity_specs(&self) -> crate::filters::arity::FilterAritySpecs {
            crate::filters::arity::FilterAritySpecs::new()
        }
    }

    fn compute_regions(db: &TestDatabase, source: &str) -> OpaqueRegions {
        let path = "test.html";
        db.add_file(path, source);
        let file = db.create_file(Utf8Path::new(path));
        let nodelist = parse_template(db, file).expect("should parse");
        compute_opaque_regions(db, nodelist)
    }

    #[test]
    fn test_opaque_regions_empty() {
        let regions = OpaqueRegions::default();
        assert!(!regions.is_opaque(0));
        assert!(!regions.is_opaque(100));
        assert!(regions.is_empty());
    }

    #[test]
    fn test_opaque_regions_basic() {
        let regions = OpaqueRegions::new(vec![Span::saturating_from_bounds_usize(10, 20)]);
        assert!(!regions.is_opaque(5));
        assert!(!regions.is_opaque(9));
        assert!(regions.is_opaque(10));
        assert!(regions.is_opaque(15));
        assert!(regions.is_opaque(19));
        assert!(!regions.is_opaque(20));
        assert!(!regions.is_opaque(25));
    }

    #[test]
    fn test_opaque_regions_multiple() {
        let regions = OpaqueRegions::new(vec![
            Span::saturating_from_bounds_usize(10, 20),
            Span::saturating_from_bounds_usize(30, 40),
        ]);
        assert!(regions.is_opaque(15));
        assert!(!regions.is_opaque(25));
        assert!(regions.is_opaque(35));
        assert!(!regions.is_opaque(45));
    }

    #[test]
    fn test_opaque_regions_sorted() {
        // Spans given out of order should still work
        let regions = OpaqueRegions::new(vec![
            Span::saturating_from_bounds_usize(30, 40),
            Span::saturating_from_bounds_usize(10, 20),
        ]);
        assert!(regions.is_opaque(15));
        assert!(regions.is_opaque(35));
        assert!(!regions.is_opaque(25));
    }

    #[test]
    fn test_verbatim_block_produces_opaque_region() {
        let db = TestDatabase::new();
        let source = "{% verbatim %}{% trans 'hello' %}{% endverbatim %}";
        let regions = compute_regions(&db, source);
        assert!(
            !regions.is_empty(),
            "verbatim block should produce an opaque region"
        );
        // trans is at byte offset 14 (after "{% verbatim %}")
        assert!(
            regions.is_opaque(14),
            "Position inside verbatim block should be opaque"
        );
    }

    #[test]
    fn test_comment_block_produces_opaque_region() {
        let db = TestDatabase::new();
        let source = "{% comment %}inner content{% endcomment %}";
        let regions = compute_regions(&db, source);
        assert!(!regions.is_empty());
        // Content is at byte offset 13 (after "{% comment %}")
        assert!(regions.is_opaque(13));
    }

    #[test]
    fn test_non_opaque_block_no_region() {
        let db = TestDatabase::new();
        let source = "{% if True %}content{% endif %}";
        let regions = compute_regions(&db, source);
        assert!(
            regions.is_empty(),
            "if block should NOT produce an opaque region"
        );
    }

    #[test]
    fn test_content_after_verbatim_not_opaque() {
        let db = TestDatabase::new();
        let source = "{% verbatim %}opaque{% endverbatim %}after";
        let regions = compute_regions(&db, source);
        // "after" starts at position 37 (past the closing "}" of endverbatim at 36)
        assert!(!regions.is_opaque(37));
    }

    #[test]
    fn test_verbatim_opaque_boundaries() {
        let db = TestDatabase::new();
        // "{% verbatim %}" = 0..14, "opaque" = 14..20, "{% endverbatim %}" = 20..37
        let source = "{% verbatim %}opaque{% endverbatim %}";
        let regions = compute_regions(&db, source);

        // The opener tag itself is NOT opaque
        assert!(!regions.is_opaque(0), "start of opener tag");
        assert!(!regions.is_opaque(13), "end of opener tag");

        // Content between the tags IS opaque
        assert!(regions.is_opaque(14), "first byte of opaque content");
        assert!(regions.is_opaque(19), "last byte of opaque content");

        // The closer tag is NOT opaque
        assert!(!regions.is_opaque(20), "start of closer tag");
        assert!(!regions.is_opaque(35), "end of closer tag");
    }
}

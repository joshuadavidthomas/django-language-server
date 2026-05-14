use djls_source::Span;
use djls_templates::tokens::TagDelimiter;
use djls_templates::Node;
use djls_templates::NodeList;

use crate::structure::BlockRole;
use crate::structure::Regions;
use crate::structure::TemplateNode;
use crate::structure::TemplateRegion;
use crate::structure::TemplateTree;
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
/// This uses a lightweight source-order scan instead of building a full
/// [`TemplateTree`]. Validation already builds a tree and can use
/// [`compute_opaque_regions_from_tree`] to avoid duplicate structure work.
pub fn compute_opaque_regions(db: &dyn Db, nodelist: NodeList<'_>) -> OpaqueRegions {
    let tag_specs = db.tag_specs();
    let mut spans = Vec::new();
    let mut stack = Vec::new();

    for node in nodelist.nodelist(db) {
        let Node::Tag { name, span, .. } = node else {
            continue;
        };

        if tag_specs.is_opener(name) {
            let body_start = span.end().saturating_add(TagDelimiter::LENGTH_U32);
            stack.push(OpaqueFrame {
                opener_name: name,
                segment_start: body_start,
                is_opaque: tag_specs.get(name).is_some_and(|spec| spec.opaque),
            });
        } else if let Some(opener_name) = tag_specs.find_opener_for_closer(name) {
            close_opaque_frame(&mut stack, &mut spans, &opener_name, *span);
        } else if tag_specs.is_intermediate(name) {
            if let Some(frame) = stack.last_mut() {
                let possible_openers = tag_specs.get_parent_tags_for_intermediate(name);
                if possible_openers
                    .iter()
                    .any(|opener| opener == frame.opener_name)
                {
                    push_opaque_segment(frame, *span, &mut spans);
                    frame.segment_start = span.end().saturating_add(TagDelimiter::LENGTH_U32);
                }
            }
        }
    }

    OpaqueRegions::new(spans)
}

/// Compute opaque regions from an already-built template tree.
///
/// Use this when the caller already needs the `TemplateTree` for structural
/// diagnostics or another semantic feature.
pub(crate) fn compute_opaque_regions_from_tree(
    db: &dyn Db,
    template_tree: TemplateTree<'_>,
) -> OpaqueRegions {
    let tag_specs = db.tag_specs();
    let regions = template_tree.regions(db);
    let mut spans = Vec::new();
    let root = &regions[template_tree.root(db)];

    collect_opaque_spans_from_region(root, regions, tag_specs, &mut spans);

    OpaqueRegions::new(spans)
}

fn close_opaque_frame(
    stack: &mut Vec<OpaqueFrame<'_>>,
    spans: &mut Vec<Span>,
    opener_name: &str,
    span: Span,
) {
    let Some(frame_idx) = stack
        .iter()
        .rposition(|frame| frame.opener_name == opener_name)
    else {
        return;
    };

    while stack.len() > frame_idx + 1 {
        stack.pop();
    }

    let Some(frame) = stack.pop() else {
        return;
    };

    push_opaque_segment(&frame, span, spans);
}

fn push_opaque_segment(frame: &OpaqueFrame<'_>, marker_span: Span, spans: &mut Vec<Span>) {
    if frame.is_opaque {
        let content_end = marker_span.start().saturating_sub(TagDelimiter::LENGTH_U32);
        spans.push(Span::saturating_from_bounds_usize(
            frame.segment_start as usize,
            content_end as usize,
        ));
    }
}

struct OpaqueFrame<'a> {
    opener_name: &'a str,
    segment_start: u32,
    is_opaque: bool,
}

fn collect_opaque_spans_from_region(
    region: &TemplateRegion,
    regions: &Regions,
    tag_specs: &TagSpecs,
    spans: &mut Vec<Span>,
) {
    for node in region.nodes() {
        if let TemplateNode::Block {
            tag, body, role, ..
        } = node
        {
            let body_region = &regions[*body];
            if matches!(role, BlockRole::Segment) {
                if let Some(spec) = tag_specs.get(tag) {
                    if spec.opaque {
                        spans.push(*body_region.span());
                    }
                }
            }
            collect_opaque_spans_from_region(body_region, regions, tag_specs, spans);
        }
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use djls_source::Span;
    use djls_templates::parse_template;

    use crate::structure::*;
    use crate::testing::TestDatabase;

    fn compute_regions(db: &TestDatabase, source: &str) -> OpaqueRegions {
        let path = "test.html";
        db.add_file(path, source);
        let file = db.create_file(Utf8Path::new(path));
        let nodelist = parse_template(db, file).expect("should parse");
        compute_opaque_regions(db, nodelist)
    }

    #[test]
    fn nodelist_and_tree_paths_match() {
        let db = TestDatabase::new();
        let path = "test.html";
        db.add_file(
            path,
            "{% verbatim %}{% if user %}raw{% endif %}{% endverbatim %}",
        );
        let file = db.create_file(Utf8Path::new(path));
        let nodelist = parse_template(&db, file).expect("should parse");
        let tree = crate::build_template_tree(&db, nodelist);

        assert_eq!(
            compute_opaque_regions(&db, nodelist),
            super::compute_opaque_regions_from_tree(&db, tree)
        );
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
        assert!(!regions.is_opaque(37));
    }

    #[test]
    fn test_verbatim_opaque_boundaries() {
        let db = TestDatabase::new();
        let source = "{% verbatim %}opaque{% endverbatim %}";
        let regions = compute_regions(&db, source);

        assert!(!regions.is_opaque(0), "start of opener tag");
        assert!(!regions.is_opaque(13), "end of opener tag");

        assert!(regions.is_opaque(14), "first byte of opaque content");
        assert!(regions.is_opaque(19), "last byte of opaque content");

        assert!(!regions.is_opaque(20), "start of closer tag");
        assert!(!regions.is_opaque(35), "end of closer tag");
    }
}

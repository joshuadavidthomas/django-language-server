use djls_source::Span;
use djls_templates::NodeList;

use crate::db::Db;
use crate::structure::build_template_tree;
use crate::structure::tree::Regions;
use crate::structure::tree::TemplateNode;
use crate::structure::tree::TemplateRegion;

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

/// Compute opaque regions for a template by projecting from the template tree.
pub fn compute_opaque_regions<'db>(db: &'db dyn Db, nodelist: NodeList<'db>) -> OpaqueRegions {
    let tree = build_template_tree(db, nodelist);
    opaque_regions_from_tree(tree.regions(db))
}

fn opaque_regions_from_tree(regions: &Regions) -> OpaqueRegions {
    let spans = regions
        .iter()
        .flat_map(TemplateRegion::nodes)
        .filter_map(|node| match node {
            TemplateNode::Opaque { body_span, .. } => Some(*body_span),
            TemplateNode::Block { .. }
            | TemplateNode::StandaloneTag { .. }
            | TemplateNode::Variable { .. }
            | TemplateNode::Comment { .. }
            | TemplateNode::Text { .. }
            | TemplateNode::Error { .. } => None,
        })
        .collect();

    OpaqueRegions::new(spans)
}

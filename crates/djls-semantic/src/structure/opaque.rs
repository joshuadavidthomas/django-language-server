use std::cmp::Ordering;

use djls_source::File;
use djls_source::Span;
use djls_templates::NodeList;

use crate::db::Db;
use crate::scoping::template_analysis_projection_for_file;
use crate::structure::build_projectless_template_tree;
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
                    Ordering::Greater
                } else if position >= span.end() {
                    Ordering::Less
                } else {
                    Ordering::Equal
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
pub fn compute_opaque_regions<'db>(
    db: &'db dyn Db,
    file: File,
    nodelist: NodeList<'db>,
) -> OpaqueRegions {
    if db.project().is_none() {
        let tree = build_projectless_template_tree(db, nodelist);
        return OpaqueRegions::new(opaque_body_spans(tree.regions(db)).collect());
    }
    OpaqueRegions::new(
        template_analysis_projection_for_file(db, file, nodelist)
            .read(|analysis| opaque_body_spans(analysis.tree(db).regions(db)).collect()),
    )
}

fn opaque_body_spans(regions: &Regions) -> impl Iterator<Item = Span> + '_ {
    regions
        .iter()
        .flat_map(TemplateRegion::nodes)
        .filter_map(|node| match node {
            TemplateNode::Opaque { body_span, .. } => Some(*body_span),
            TemplateNode::Block { .. }
            | TemplateNode::StandaloneTag { .. }
            | TemplateNode::Variable { .. }
            | TemplateNode::Comment { .. }
            | TemplateNode::Error { .. } => None,
        })
}

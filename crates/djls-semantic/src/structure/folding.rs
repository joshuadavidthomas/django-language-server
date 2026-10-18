use djls_source::Span;

use crate::db::Db;
use crate::structure::BlockRole;
use crate::structure::RegionId;
use crate::structure::Regions;
use crate::structure::TemplateNode;
use crate::structure::TemplateTree;
use crate::tags::BodyAnalysis;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TemplateFold {
    pub span: Span,
    pub kind: TemplateFoldKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TemplateFoldKind {
    Region,
    Comment,
}

#[salsa::tracked(returns(ref))]
pub fn build_template_folds(db: &dyn Db, tree: TemplateTree<'_>) -> Vec<TemplateFold> {
    let mut folds = Vec::new();
    collect_folds_for_region(tree.regions(db), tree.root(db), &mut folds);
    folds
}

fn collect_folds_for_region(regions: &Regions, region: RegionId, folds: &mut Vec<TemplateFold>) {
    for node in regions.get(region).nodes() {
        match node {
            TemplateNode::Block {
                full_span,
                body,
                role: BlockRole::Opener,
                ..
            } => {
                let end = regions.get(*body).span().end();
                if end > full_span.end() {
                    folds.push(TemplateFold {
                        span: Span::saturating_from_bounds_usize(
                            full_span.start_usize(),
                            end as usize,
                        ),
                        kind: TemplateFoldKind::Region,
                    });
                }
                collect_folds_for_region(regions, *body, folds);
            }
            TemplateNode::Block {
                body,
                role: BlockRole::Segment,
                ..
            } => {
                collect_folds_for_region(regions, *body, folds);
            }
            TemplateNode::Opaque {
                full_span,
                body_analysis,
                ..
            } => {
                folds.push(TemplateFold {
                    span: *full_span,
                    kind: TemplateFoldKind::from_body_analysis(*body_analysis),
                });
            }
            TemplateNode::StandaloneTag { .. }
            | TemplateNode::Variable { .. }
            | TemplateNode::Comment { .. }
            | TemplateNode::Text { .. }
            | TemplateNode::Error { .. } => {}
        }
    }
}

impl TemplateFoldKind {
    /// A body the compile function discards is a comment; a verbatim body is rendered.
    fn from_body_analysis(body_analysis: BodyAnalysis) -> Self {
        match body_analysis {
            BodyAnalysis::Opaque => Self::Comment,
            BodyAnalysis::Analyze | BodyAnalysis::Verbatim => Self::Region,
        }
    }
}

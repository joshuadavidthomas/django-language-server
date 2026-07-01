use djls_source::Span;

use crate::db::Db;
use crate::structure::BlockRole;
use crate::structure::RegionId;
use crate::structure::Regions;
use crate::structure::TemplateNode;
use crate::structure::TemplateTree;

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
                tag,
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
                        kind: TemplateFoldKind::from_tag_name(tag),
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
            TemplateNode::Opaque { tag, full_span, .. } => {
                folds.push(TemplateFold {
                    span: *full_span,
                    kind: TemplateFoldKind::from_tag_name(tag),
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
    fn from_tag_name(name: &str) -> Self {
        match name {
            "comment" => Self::Comment,
            _ => Self::Region,
        }
    }
}

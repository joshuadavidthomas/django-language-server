use djls_source::File;
use djls_source::Span;
use djls_templates::NodeList;

use crate::db::Db;
use crate::layout::MapSpans;
use crate::scoping::template_analysis_projection_for_file;
use crate::structure::BlockRole;
use crate::structure::RegionId;
use crate::structure::Regions;
use crate::structure::TemplateNode;
use crate::structure::TemplateTree;
use crate::structure::build_projectless_template_tree;
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
fn build_template_folds(db: &dyn Db, tree: TemplateTree<'_>) -> Vec<TemplateFold> {
    let mut folds = Vec::new();
    collect_folds_for_region(tree.regions(db), tree.root(db), &mut folds);
    folds
}

/// Folds for `file` in source positions, read from the analysis tree rather than a copy of it.
#[salsa::tracked(returns(ref))]
pub fn build_template_folds_for_file<'db>(
    db: &'db dyn Db,
    file: File,
    nodelist: NodeList<'db>,
) -> Vec<TemplateFold> {
    if db.project().is_none() {
        return build_template_folds(db, build_projectless_template_tree(db, nodelist)).clone();
    }
    template_analysis_projection_for_file(db, file, nodelist).read(|analysis| {
        let tree = analysis.tree(db);
        let mut folds = Vec::new();
        collect_folds_for_region(tree.regions(db), tree.root(db), &mut folds);
        folds
    })
}

impl MapSpans for TemplateFold {
    fn map_spans(&mut self, map: &mut impl FnMut(Span) -> Span) {
        self.span.map_spans(map);
    }
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

use djls_semantic::structure::forest::SemanticNode;
use djls_semantic::structure::forest::SemanticSegment;
use djls_source::File;
use djls_source::Span;
use djls_templates::Node;
use tower_lsp_server::ls_types;

use crate::ext::FoldingRangeKindExt;
use crate::ext::SpanExt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum FoldKind {
    Region,
    Comment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct FoldSpan {
    span: Span,
    kind: FoldKind,
}

#[must_use]
pub fn collect_folding_ranges(
    db: &dyn djls_semantic::Db,
    file: File,
) -> Vec<ls_types::FoldingRange> {
    let Some(nodelist) = djls_templates::parse_template(db, file) else {
        return Vec::new();
    };

    let block_tree = djls_semantic::build_block_tree(db, nodelist);
    let forest = djls_semantic::build_semantic_forest(db, block_tree, nodelist);

    let mut folds = Vec::new();
    for root in forest.roots(db) {
        collect_node_folds(root, &mut folds);
    }

    for node in nodelist.nodelist(db) {
        if let Node::Comment { .. } = node {
            folds.push(FoldSpan {
                span: node.full_span(),
                kind: FoldKind::Comment,
            });
        }
    }

    folds.sort_by_key(|fold| (fold.span.start(), fold.span.end(), fold.kind_key()));
    folds.dedup();

    let line_index = file.line_index(db);
    folds
        .into_iter()
        .filter_map(|fold| {
            let range = fold.span.to_lsp_range(line_index);

            if range.start.line >= range.end.line {
                return None;
            }

            Some(ls_types::FoldingRange {
                start_line: range.start.line,
                start_character: Some(range.start.character),
                end_line: range.end.line,
                end_character: Some(range.end.character),
                kind: Some(fold.kind.to_lsp_kind()),
                collapsed_text: None,
            })
        })
        .collect()
}

fn collect_node_folds(node: &SemanticNode, folds: &mut Vec<FoldSpan>) {
    let SemanticNode::Tag {
        marker_span,
        segments,
        ..
    } = node
    else {
        return;
    };

    if let Some(span) = fold_span(*marker_span, segments) {
        folds.push(FoldSpan {
            span,
            kind: FoldKind::Region,
        });
    }

    for segment in segments {
        for child in &segment.children {
            collect_node_folds(child, folds);
        }
    }
}

fn fold_span(marker_span: Span, segments: &[SemanticSegment]) -> Option<Span> {
    let end = segments
        .iter()
        .map(|segment| segment.content_span.end())
        .max()?;

    if marker_span.start() >= end {
        return None;
    }

    Some(Span::saturating_from_bounds_usize(
        marker_span.start() as usize,
        end as usize,
    ))
}

impl FoldSpan {
    fn kind_key(self) -> u8 {
        match self.kind {
            FoldKind::Region => 0,
            FoldKind::Comment => 1,
        }
    }
}

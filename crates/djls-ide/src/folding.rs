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
    Imports,
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

    let nodes = nodelist.nodelist(db);
    for node in nodes {
        if let Node::Comment { .. } = node {
            folds.push(FoldSpan {
                span: node.full_span(),
                kind: FoldKind::Comment,
            });
        }
    }
    collect_import_folds(nodes, file.source(db).as_str(), &mut folds);

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
            kind: fold_kind(node),
        });
    }

    for segment in segments {
        for child in &segment.children {
            collect_node_folds(child, folds);
        }
    }
}

fn fold_kind(node: &SemanticNode) -> FoldKind {
    match node {
        SemanticNode::Tag { name, .. } if name == "comment" => FoldKind::Comment,
        SemanticNode::Tag { .. } | SemanticNode::Leaf { .. } => FoldKind::Region,
    }
}

fn collect_import_folds(nodes: &[Node], source: &str, folds: &mut Vec<FoldSpan>) {
    let mut group = ImportGroup::default();

    for node in nodes {
        match node {
            Node::Tag { name, .. } if name == "extends" => {
                group.push(folds);
                group.start = Some(node.full_span().start());
                group.end = Some(node.full_span().end());
            }
            Node::Tag { name, .. } if name == "load" => {
                group.start.get_or_insert_with(|| node.full_span().start());
                group.end = Some(node.full_span().end());
                group.has_load = true;
            }
            Node::Text { span } if is_whitespace(source, *span) => {}
            _ => group.push(folds),
        }
    }

    group.push(folds);
}

#[derive(Default)]
struct ImportGroup {
    start: Option<u32>,
    end: Option<u32>,
    has_load: bool,
}

impl ImportGroup {
    fn push(&mut self, folds: &mut Vec<FoldSpan>) {
        let (Some(start), Some(end)) = (self.start.take(), self.end.take()) else {
            return;
        };
        let has_load = std::mem::take(&mut self.has_load);

        if !has_load || start >= end {
            return;
        }

        folds.push(FoldSpan {
            span: Span::saturating_from_bounds_usize(start as usize, end as usize),
            kind: FoldKind::Imports,
        });
    }
}

fn is_whitespace(source: &str, span: Span) -> bool {
    source
        .get(span.start_usize()..span.end() as usize)
        .is_some_and(|text| text.trim().is_empty())
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
            FoldKind::Imports => 2,
        }
    }
}

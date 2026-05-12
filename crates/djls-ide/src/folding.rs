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

impl FoldKind {
    fn sort_key(self) -> u8 {
        match self {
            Self::Region => 0,
            Self::Comment => 1,
            Self::Imports => 2,
        }
    }
}

impl From<&SemanticNode> for FoldKind {
    fn from(node: &SemanticNode) -> Self {
        match node {
            SemanticNode::Tag { name, .. } if name == "comment" => Self::Comment,
            SemanticNode::Tag { .. } | SemanticNode::Leaf { .. } => Self::Region,
        }
    }
}

impl FoldSpan {
    fn sort_key(self) -> (u32, u32, u8) {
        (self.span.start(), self.span.end(), self.kind.sort_key())
    }
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

    let mut folds = collect_structure_folds(forest.roots(db));
    let template_folds =
        collect_template_node_folds(nodelist.nodelist(db), file.source(db).as_str());
    folds.extend(template_folds);

    folds.sort_by_key(|fold| fold.sort_key());
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

fn collect_structure_folds(roots: &[SemanticNode]) -> Vec<FoldSpan> {
    let mut folds = Vec::new();
    for root in roots {
        collect_node_folds(root, &mut folds);
    }
    folds
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
            kind: FoldKind::from(node),
        });
    }

    for segment in segments {
        for child in &segment.children {
            collect_node_folds(child, folds);
        }
    }
}

fn collect_template_node_folds(nodes: &[Node], source: &str) -> Vec<FoldSpan> {
    let mut folds = Vec::new();
    let mut import = ImportHeader::Empty;

    for node in nodes {
        match node {
            Node::Comment { .. } => {
                folds.extend(import.take_fold());
                folds.push(FoldSpan {
                    span: node.full_span(),
                    kind: FoldKind::Comment,
                });
            }
            Node::Tag { name, .. } if name == "extends" => {
                folds.extend(import.take_fold());
                import = ImportHeader::Extends {
                    start: node.full_span().start(),
                };
            }
            Node::Tag { name, .. } if name == "load" => {
                import.include_load(node.full_span());
            }
            Node::Text { span }
                if source
                    .get(span.start_usize()..span.end() as usize)
                    .is_some_and(|text| text.trim().is_empty()) => {}
            _ => folds.extend(import.take_fold()),
        }
    }

    folds.extend(import.take_fold());
    folds
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImportHeader {
    Empty,
    Extends { start: u32 },
    Imports { start: u32, end: u32 },
}

impl ImportHeader {
    fn include_load(&mut self, span: Span) {
        let end = span.end();
        *self = match *self {
            Self::Empty => Self::Imports {
                start: span.start(),
                end,
            },
            Self::Extends { start } | Self::Imports { start, .. } => Self::Imports { start, end },
        };
    }

    fn take_fold(&mut self) -> Option<FoldSpan> {
        let Self::Imports { start, end } = std::mem::replace(self, Self::Empty) else {
            return None;
        };

        if start >= end {
            return None;
        }

        Some(FoldSpan {
            span: Span::saturating_from_bounds_usize(start as usize, end as usize),
            kind: FoldKind::Imports,
        })
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

use djls_semantic::structure::forest::SemanticNode;
use djls_source::File;
use djls_source::LineIndex;
use djls_source::Span;
use djls_templates::Node;
use tower_lsp_server::ls_types;

use crate::ext::FoldingRangeKindExt;
use crate::ext::SpanExt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct FoldSpan {
    span: Span,
    kind: FoldKind,
}

impl FoldSpan {
    fn sort_key(self) -> (u32, u32, u8) {
        (self.span.start(), self.span.end(), self.kind.sort_key())
    }

    fn into_lsp(self, line_index: &LineIndex) -> Option<ls_types::FoldingRange> {
        let range = self.span.to_lsp_range(line_index);

        if range.start.line >= range.end.line {
            return None;
        }

        Some(ls_types::FoldingRange {
            start_line: range.start.line,
            start_character: Some(range.start.character),
            end_line: range.end.line,
            end_character: Some(range.end.character),
            kind: Some(self.kind.to_lsp_kind()),
            collapsed_text: None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum FoldKind {
    Region,
    Comment,
    Imports,
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

#[must_use]
pub fn collect_folding_ranges(
    db: &dyn djls_semantic::Db,
    file: File,
) -> Vec<ls_types::FoldingRange> {
    let Some(nodelist) = djls_templates::parse_template(db, file) else {
        return Vec::new();
    };

    let mut folds = Vec::new();

    append_semantic_folds(db, nodelist, &mut folds);
    append_header_folds(db, file, nodelist, &mut folds);

    folds.sort_by_key(|fold| fold.sort_key());
    folds.dedup();

    let line_index = file.line_index(db);
    folds
        .into_iter()
        .filter_map(|fold| fold.into_lsp(line_index))
        .collect()
}

fn append_semantic_folds(
    db: &dyn djls_semantic::Db,
    nodelist: djls_templates::NodeList<'_>,
    folds: &mut Vec<FoldSpan>,
) {
    let block_tree = djls_semantic::build_block_tree(db, nodelist);
    let forest = djls_semantic::build_semantic_forest(db, block_tree, nodelist);
    let mut stack: Vec<_> = forest.roots(db).iter().collect();

    while let Some(node) = stack.pop() {
        let SemanticNode::Tag {
            marker_span,
            segments,
            ..
        } = node
        else {
            continue;
        };

        if let Some(end) = segments
            .iter()
            .map(|segment| segment.content_span.end())
            .max()
        {
            if marker_span.start() < end {
                folds.push(FoldSpan {
                    span: Span::saturating_from_bounds_usize(
                        marker_span.start() as usize,
                        end as usize,
                    ),
                    kind: FoldKind::from(node),
                });
            }
        }

        for segment in segments {
            stack.extend(&segment.children);
        }
    }
}

fn append_header_folds(
    db: &dyn djls_semantic::Db,
    file: File,
    nodelist: djls_templates::NodeList<'_>,
    folds: &mut Vec<FoldSpan>,
) {
    let source = file.source(db);
    let mut import: Option<ImportHeader> = None;

    for node in nodelist.nodelist(db) {
        match node {
            Node::Comment { .. } => {
                if let Some(fold) = import.take().and_then(ImportHeader::into_fold) {
                    folds.push(fold);
                }
                folds.push(FoldSpan {
                    span: node.full_span(),
                    kind: FoldKind::Comment,
                });
            }
            Node::Tag { name, .. } if name == "extends" => {
                if let Some(fold) = import.take().and_then(ImportHeader::into_fold) {
                    folds.push(fold);
                }
                import = Some(ImportHeader::Extends {
                    start: node.full_span().start(),
                });
            }
            Node::Tag { name, .. } if name == "load" => {
                import = Some(match import.take() {
                    Some(header) => header.with_load(node.full_span()),
                    None => ImportHeader::from_load(node.full_span()),
                });
            }
            Node::Text { span }
                if source
                    .as_str()
                    .get(span.start_usize()..span.end() as usize)
                    .is_some_and(|text| text.trim().is_empty()) => {}
            _ => {
                if let Some(fold) = import.take().and_then(ImportHeader::into_fold) {
                    folds.push(fold);
                }
            }
        }
    }

    if let Some(fold) = import.and_then(ImportHeader::into_fold) {
        folds.push(fold);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImportHeader {
    Extends { start: u32 },
    Imports { start: u32, end: u32 },
}

impl ImportHeader {
    fn from_load(span: Span) -> Self {
        Self::Imports {
            start: span.start(),
            end: span.end(),
        }
    }

    fn with_load(self, span: Span) -> Self {
        match self {
            Self::Extends { start } | Self::Imports { start, .. } => Self::Imports {
                start,
                end: span.end(),
            },
        }
    }

    fn into_fold(self) -> Option<FoldSpan> {
        let Self::Imports { start, end } = self else {
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

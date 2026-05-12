use djls_semantic::structure::forest::SemanticNode;
use djls_semantic::structure::forest::SemanticSegment;
use djls_source::File;
use djls_source::Span;
use djls_templates::Node;
use tower_lsp_server::ls_types;

use crate::ext::FoldingRangeKindExt;
use crate::ext::SpanExt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Fold {
    Region(Span),
    Comment(Span),
    Imports(Span),
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
            folds.push(Fold::Comment(node.full_span()));
        }
    }
    collect_import_folds(nodes, file.source(db).as_str(), &mut folds);

    folds.sort_by_key(|fold| {
        let span = fold.span();
        (span.start(), span.end(), fold.kind_key())
    });
    folds.dedup();

    let line_index = file.line_index(db);
    folds
        .into_iter()
        .filter_map(|fold| {
            let range = fold.span().to_lsp_range(line_index);

            if range.start.line >= range.end.line {
                return None;
            }

            Some(ls_types::FoldingRange {
                start_line: range.start.line,
                start_character: Some(range.start.character),
                end_line: range.end.line,
                end_character: Some(range.end.character),
                kind: Some(fold.to_lsp_kind()),
                collapsed_text: None,
            })
        })
        .collect()
}

fn collect_node_folds(node: &SemanticNode, folds: &mut Vec<Fold>) {
    let SemanticNode::Tag {
        marker_span,
        segments,
        ..
    } = node
    else {
        return;
    };

    if let Some(span) = fold_span(*marker_span, segments) {
        folds.push(fold_for_node(node, span));
    }

    for segment in segments {
        for child in &segment.children {
            collect_node_folds(child, folds);
        }
    }
}

fn fold_for_node(node: &SemanticNode, span: Span) -> Fold {
    match node {
        SemanticNode::Tag { name, .. } if name == "comment" => Fold::Comment(span),
        SemanticNode::Tag { .. } | SemanticNode::Leaf { .. } => Fold::Region(span),
    }
}

fn collect_import_folds(nodes: &[Node], source: &str, folds: &mut Vec<Fold>) {
    let mut import = PendingImport::Empty;

    for node in nodes {
        match node {
            Node::Tag { name, .. } if name == "extends" => {
                import.push(folds);
                import = PendingImport::Extends {
                    start: node.full_span().start(),
                };
            }
            Node::Tag { name, .. } if name == "load" => {
                import.include_load(node.full_span());
            }
            Node::Text { span } if is_whitespace(source, *span) => {}
            _ => import.push(folds),
        }
    }

    import.push(folds);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingImport {
    Empty,
    Extends { start: u32 },
    Imports { start: u32, end: u32 },
}

impl PendingImport {
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

    fn push(&mut self, folds: &mut Vec<Fold>) {
        let Self::Imports { start, end } = *self else {
            *self = Self::Empty;
            return;
        };

        *self = Self::Empty;

        if start >= end {
            return;
        }

        folds.push(Fold::Imports(Span::saturating_from_bounds_usize(
            start as usize,
            end as usize,
        )));
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

impl Fold {
    fn span(self) -> Span {
        match self {
            Self::Region(span) | Self::Comment(span) | Self::Imports(span) => span,
        }
    }

    fn kind_key(self) -> u8 {
        match self {
            Self::Region(_) => 0,
            Self::Comment(_) => 1,
            Self::Imports(_) => 2,
        }
    }
}

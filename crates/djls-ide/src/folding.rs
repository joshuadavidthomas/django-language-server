use djls_semantic::structure::forest::SemanticNode;
use djls_source::File;
use djls_source::Span;
use djls_templates::Node;
use tower_lsp_server::ls_types;

use crate::ext::FoldSpanExt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FoldSpan {
    pub(crate) span: Span,
    pub(crate) kind: FoldKind,
}

impl FoldSpan {
    fn from_bounds(start: u32, end: u32, kind: FoldKind) -> Option<Self> {
        if start >= end {
            return None;
        }

        Some(Self {
            span: Span::saturating_from_bounds_usize(start as usize, end as usize),
            kind,
        })
    }

    fn comment(span: Span) -> Self {
        Self {
            span,
            kind: FoldKind::Comment,
        }
    }

    fn imports(start: u32, end: u32) -> Option<Self> {
        Self::from_bounds(start, end, FoldKind::Imports)
    }

    fn sort_key(self) -> (u32, u32, u8) {
        (self.span.start(), self.span.end(), self.kind.sort_key())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum FoldKind {
    Region,
    Comment,
    Imports,
}

impl FoldKind {
    fn from_semantic_tag_name(name: &str) -> Self {
        match name {
            "comment" => Self::Comment,
            _ => Self::Region,
        }
    }

    fn sort_key(self) -> u8 {
        match self {
            Self::Region => 0,
            Self::Comment => 1,
            Self::Imports => 2,
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
        .filter_map(|fold| fold.to_lsp_folding_range(line_index))
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
            name,
            marker_span,
            segments,
            ..
        } = node
        else {
            continue;
        };

        if let Some(fold) = segments
            .iter()
            .map(|segment| segment.content_span.end())
            .max()
            .and_then(|end| {
                FoldSpan::from_bounds(
                    marker_span.start(),
                    end,
                    FoldKind::from_semantic_tag_name(name),
                )
            })
        {
            folds.push(fold);
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
    let mut imports = ImportHeaderCandidate::None;

    for node in nodelist.nodelist(db) {
        match HeaderItem::from_node(node, source.as_str()) {
            HeaderItem::Comment(span) => {
                imports.finish_into(folds);
                folds.push(FoldSpan::comment(span));
            }
            HeaderItem::Extends(span) => {
                imports.finish_into(folds);
                imports.begin_with_extends(span);
            }
            HeaderItem::Load(span) => {
                imports.include_load(span);
            }
            HeaderItem::Whitespace => {}
            HeaderItem::Boundary => imports.finish_into(folds),
        }
    }

    imports.finish_into(folds);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeaderItem {
    Comment(Span),
    Extends(Span),
    Load(Span),
    Whitespace,
    Boundary,
}

impl HeaderItem {
    fn from_node(node: &Node, source: &str) -> Self {
        match node {
            Node::Comment { .. } => Self::Comment(node.full_span()),
            Node::Tag { name, .. } if name == "extends" => Self::Extends(node.full_span()),
            Node::Tag { name, .. } if name == "load" => Self::Load(node.full_span()),
            Node::Text { span }
                if source
                    .get(span.start_usize()..span.end() as usize)
                    .is_some_and(|text| text.trim().is_empty()) =>
            {
                Self::Whitespace
            }
            Node::Tag { .. } | Node::Text { .. } | Node::Variable { .. } | Node::Error { .. } => {
                Self::Boundary
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImportHeaderCandidate {
    None,
    Started {
        start: u32,
        last_load_end: Option<u32>,
    },
}

impl ImportHeaderCandidate {
    fn begin_with_extends(&mut self, span: Span) {
        *self = Self::Started {
            start: span.start(),
            last_load_end: None,
        };
    }

    fn include_load(&mut self, span: Span) {
        match self {
            Self::None => {
                *self = Self::Started {
                    start: span.start(),
                    last_load_end: Some(span.end()),
                };
            }
            Self::Started { last_load_end, .. } => {
                *last_load_end = Some(span.end());
            }
        }
    }

    fn finish(&mut self) -> Option<FoldSpan> {
        let finished = std::mem::replace(self, Self::None);

        match finished {
            Self::Started {
                start,
                last_load_end: Some(end),
            } => FoldSpan::imports(start, end),
            Self::None
            | Self::Started {
                last_load_end: None,
                ..
            } => None,
        }
    }

    fn finish_into(&mut self, folds: &mut Vec<FoldSpan>) {
        if let Some(fold) = self.finish() {
            folds.push(fold);
        }
    }
}

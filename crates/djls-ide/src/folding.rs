use djls_semantic::TagClass;
use djls_semantic::TagIndex;
use djls_source::File;
use djls_source::Span;
use djls_templates::Node;
use tower_lsp_server::ls_types;

use crate::ext::FoldSpanExt;

#[must_use]
pub fn collect_folding_ranges(
    db: &dyn djls_semantic::Db,
    file: File,
) -> Vec<ls_types::FoldingRange> {
    let Some(nodelist) = djls_templates::parse_template(db, file) else {
        return Vec::new();
    };

    let source = file.source(db);
    let tag_index = djls_semantic::compute_tag_index(db);
    let folds = FoldSpans::collect(nodelist.nodelist(db), source.as_str(), tag_index).into_vec();

    let line_index = file.line_index(db);
    folds
        .into_iter()
        .filter_map(|fold| fold.to_lsp_folding_range(line_index))
        .collect()
}

#[derive(Default)]
struct FoldSpans(Vec<FoldSpan>);

impl FoldSpans {
    fn collect(nodes: &[Node], source: &str, tag_index: &TagIndex) -> Self {
        let mut folds = Self::default();
        folds.collect_semantic(nodes, tag_index);
        folds.collect_header(nodes, source);
        folds
    }

    fn collect_semantic(&mut self, nodes: &[Node], tag_index: &TagIndex) {
        let mut stack = Vec::new();

        for node in nodes {
            let Node::Tag { name, .. } = node else {
                continue;
            };

            match tag_index.classify(name) {
                TagClass::Opener => {
                    stack.push((name.as_str(), node.full_span()));
                }
                TagClass::Closer { opener_name } => {
                    let Some(open_idx) = stack
                        .iter()
                        .rposition(|(stacked_opener_name, _)| *stacked_opener_name == opener_name)
                    else {
                        continue;
                    };

                    let (matched_opener_name, opener_span) = stack.remove(open_idx);
                    self.push_bounds(
                        opener_span.start(),
                        node.full_span().end(),
                        FoldKind::from_semantic_tag_name(matched_opener_name),
                    );
                }
                TagClass::Intermediate { .. } | TagClass::Unknown => {}
            }
        }
    }

    fn collect_header(&mut self, nodes: &[Node], source: &str) {
        let mut imports = ImportHeaderCandidate::None;

        for node in nodes {
            match HeaderItem::from_node(node, source) {
                HeaderItem::Comment(span) => {
                    self.push_imports(imports.finish());
                    self.push(FoldSpan::comment(span));
                }
                HeaderItem::Extends(span) => {
                    self.push_imports(imports.finish());
                    imports.begin_with_extends(span);
                }
                HeaderItem::Load(span) => {
                    imports.include_load(span);
                }
                HeaderItem::Whitespace => {}
                HeaderItem::Boundary => self.push_imports(imports.finish()),
            }
        }

        self.push_imports(imports.finish());
    }

    fn push(&mut self, fold: FoldSpan) {
        self.0.push(fold);
    }

    fn push_bounds(&mut self, start: u32, end: u32, kind: FoldKind) {
        if let Some(fold) = FoldSpan::from_bounds(start, end, kind) {
            self.push(fold);
        }
    }

    fn push_imports(&mut self, fold: Option<FoldSpan>) {
        if let Some(fold) = fold {
            self.push(fold);
        }
    }

    fn into_vec(mut self) -> Vec<FoldSpan> {
        self.0.sort_by_key(|fold| fold.sort_key());
        self.0.dedup();
        self.0
    }
}

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
}

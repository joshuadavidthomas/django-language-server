use djls_semantic::TemplateFold;
use djls_semantic::TemplateFoldKind;
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
    let template_tree = djls_semantic::build_template_tree(db, nodelist);
    let semantic_folds = djls_semantic::build_template_folds(db, template_tree);
    let folds =
        FoldSpans::collect(semantic_folds, nodelist.nodelist(db), source.as_str()).into_vec();

    let line_index = file.line_index(db);
    folds
        .into_iter()
        .filter_map(|fold| fold.to_lsp_folding_range(line_index))
        .collect()
}

#[derive(Default)]
struct FoldSpans(Vec<FoldSpan>);

impl FoldSpans {
    fn collect(semantic_folds: &[TemplateFold], nodes: &[Node], source: &str) -> Self {
        let mut folds = Self::default();
        for fold in semantic_folds {
            folds.push((*fold).into());
        }

        let mut import_header = ImportHeaderFold::None;
        for node in nodes {
            match node {
                Node::Comment { .. } => {
                    folds.push_imports(import_header.finish());
                    folds.push(FoldSpan::comment(node.full_span()));
                }
                Node::Tag { name, .. } if name == "extends" => {
                    folds.push_imports(import_header.finish());
                    import_header.start_at_extends(node.full_span());
                }
                Node::Tag { name, .. } if name == "load" => {
                    import_header.include_load(node.full_span());
                }
                Node::Text { span }
                    if source
                        .get(span.start_usize()..span.end_usize())
                        .is_some_and(|text| text.trim().is_empty()) => {}
                Node::Tag { .. }
                | Node::Text { .. }
                | Node::Variable { .. }
                | Node::Error { .. } => {
                    folds.push_imports(import_header.finish());
                }
            }
        }
        folds.push_imports(import_header.finish());

        folds
    }

    fn push(&mut self, fold: FoldSpan) {
        self.0.push(fold);
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

impl From<TemplateFold> for FoldSpan {
    fn from(fold: TemplateFold) -> Self {
        Self {
            span: fold.span,
            kind: fold.kind.into(),
        }
    }
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

impl From<TemplateFoldKind> for FoldKind {
    fn from(kind: TemplateFoldKind) -> Self {
        match kind {
            TemplateFoldKind::Region => Self::Region,
            TemplateFoldKind::Comment => Self::Comment,
        }
    }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImportHeaderFold {
    None,
    ExtendsOnly { start: u32 },
    Imports { start: u32, end: u32 },
}

impl ImportHeaderFold {
    fn start_at_extends(&mut self, span: Span) {
        *self = Self::ExtendsOnly {
            start: span.start(),
        };
    }

    fn include_load(&mut self, span: Span) {
        match self {
            Self::None => {
                *self = Self::Imports {
                    start: span.start(),
                    end: span.end(),
                };
            }
            Self::ExtendsOnly { start } => {
                *self = Self::Imports {
                    start: *start,
                    end: span.end(),
                };
            }
            Self::Imports { end, .. } => {
                *end = span.end();
            }
        }
    }

    fn finish(&mut self) -> Option<FoldSpan> {
        match std::mem::replace(self, Self::None) {
            Self::Imports { start, end } => FoldSpan::imports(start, end),
            Self::None | Self::ExtendsOnly { .. } => None,
        }
    }
}

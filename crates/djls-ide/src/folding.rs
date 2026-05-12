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
        match FoldableTag::from_name(name) {
            Some(FoldableTag::Comment) => Self::Comment,
            Some(FoldableTag::Extends | FoldableTag::Load) | None => Self::Region,
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
enum FoldableTag {
    Comment,
    Extends,
    Load,
}

impl FoldableTag {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "comment" => Some(Self::Comment),
            "extends" => Some(Self::Extends),
            "load" => Some(Self::Load),
            _ => None,
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
    let mut import: Option<PendingImportHeader> = None;

    for node in nodelist.nodelist(db) {
        match node {
            Node::Comment { .. } => {
                flush_import_header(&mut import, folds);
                folds.push(FoldSpan::comment(node.full_span()));
            }
            Node::Tag { name, .. } => match FoldableTag::from_name(name) {
                Some(FoldableTag::Extends) => {
                    flush_import_header(&mut import, folds);
                    import = Some(PendingImportHeader::from_extends(node.full_span()));
                }
                Some(FoldableTag::Load) => {
                    import = Some(match import.take() {
                        Some(header) => header.with_load(node.full_span()),
                        None => PendingImportHeader::from_load(node.full_span()),
                    });
                }
                Some(FoldableTag::Comment) | None => flush_import_header(&mut import, folds),
            },
            Node::Text { span }
                if source
                    .as_str()
                    .get(span.start_usize()..span.end() as usize)
                    .is_some_and(|text| text.trim().is_empty()) => {}
            _ => flush_import_header(&mut import, folds),
        }
    }

    flush_import_header(&mut import, folds);
}

fn flush_import_header(import: &mut Option<PendingImportHeader>, folds: &mut Vec<FoldSpan>) {
    if let Some(fold) = import.take().and_then(PendingImportHeader::into_fold) {
        folds.push(fold);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingImportHeader {
    ExtendsOnly { start: u32 },
    Imports { start: u32, end: u32 },
}

impl PendingImportHeader {
    fn from_extends(span: Span) -> Self {
        Self::ExtendsOnly {
            start: span.start(),
        }
    }

    fn from_load(span: Span) -> Self {
        Self::Imports {
            start: span.start(),
            end: span.end(),
        }
    }

    fn with_load(self, span: Span) -> Self {
        match self {
            Self::ExtendsOnly { start } | Self::Imports { start, .. } => Self::Imports {
                start,
                end: span.end(),
            },
        }
    }

    fn into_fold(self) -> Option<FoldSpan> {
        let Self::Imports { start, end } = self else {
            return None;
        };

        FoldSpan::imports(start, end)
    }
}

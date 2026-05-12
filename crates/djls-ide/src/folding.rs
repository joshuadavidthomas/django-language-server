use djls_semantic::TagSpecs;
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

    let source = file.source(db);
    let folds = collect_fold_spans_impl(nodelist.nodelist(db), source.as_str(), db.tag_specs());

    let line_index = file.line_index(db);
    folds
        .into_iter()
        .filter_map(|fold| fold.to_lsp_folding_range(line_index))
        .collect()
}

fn collect_fold_spans_impl(nodes: &[Node], source: &str, tag_specs: &TagSpecs) -> Vec<FoldSpan> {
    let mut folds = Vec::new();

    append_semantic_folds(nodes, tag_specs, &mut folds);
    append_header_folds(nodes, source, &mut folds);

    folds.sort_by_key(|fold| fold.sort_key());
    folds.dedup();
    folds
}

fn append_semantic_folds(nodes: &[Node], tag_specs: &TagSpecs, folds: &mut Vec<FoldSpan>) {
    let mut stack = Vec::new();

    for node in nodes {
        let Node::Tag { name, .. } = node else {
            continue;
        };

        if tag_specs.is_opener(name) {
            stack.push((name.as_str(), node.full_span()));
            continue;
        }

        let Some(open_idx) = stack.iter().rposition(|(opener_name, _)| {
            tag_specs
                .get(*opener_name)
                .and_then(|spec| spec.end_tag.as_ref())
                .is_some_and(|end_tag| end_tag.name.as_ref() == name)
        }) else {
            continue;
        };

        let (opener_name, opener_span) = stack.remove(open_idx);
        if let Some(fold) = FoldSpan::from_bounds(
            opener_span.start(),
            node.full_span().end(),
            FoldKind::from_semantic_tag_name(opener_name),
        ) {
            folds.push(fold);
        }
    }
}

fn append_header_folds(nodes: &[Node], source: &str, folds: &mut Vec<FoldSpan>) {
    let mut imports = ImportHeaderCandidate::None;

    for node in nodes {
        match HeaderItem::from_node(node, source) {
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

#[cfg(test)]
mod tests {
    use djls_source::LineIndex;
    use djls_templates::parse_template_impl;

    use super::*;

    #[test]
    fn folding_ranges_include_top_level_and_nested_template_blocks() {
        let source = r"{% load static %}

<!DOCTYPE html>
<title>
  {% block title %}
    Django Test App
  {% endblock %}
</title>
<main>
  {% block content %}
    {% if items %}
      <ul>
        {% for item in items %}
          <li>{{ item.name }}</li>
        {% endfor %}
      </ul>
    {% else %}
      <p>No items found.</p>
    {% endif %}
  {% endblock %}
</main>
";
        let (nodes, errors) = parse_template_impl(source);
        assert!(errors.is_empty());

        let line_index = LineIndex::from(source);
        let ranges: Vec<_> =
            collect_fold_spans_impl(&nodes, source, &djls_semantic::builtin_tag_specs())
                .into_iter()
                .filter_map(|fold| fold.to_lsp_folding_range(&line_index))
                .collect();

        assert!(ranges.iter().any(|range| {
            range.start_line == 4
                && range.end_line == 6
                && range.kind == Some(ls_types::FoldingRangeKind::Region)
        }));
        assert!(ranges.iter().any(|range| {
            range.start_line == 9
                && range.end_line == 19
                && range.kind == Some(ls_types::FoldingRangeKind::Region)
        }));
        assert!(ranges.iter().any(|range| {
            range.start_line == 10
                && range.end_line == 18
                && range.kind == Some(ls_types::FoldingRangeKind::Region)
        }));
        assert!(ranges.iter().any(|range| {
            range.start_line == 12
                && range.end_line == 14
                && range.kind == Some(ls_types::FoldingRangeKind::Region)
        }));
    }
}

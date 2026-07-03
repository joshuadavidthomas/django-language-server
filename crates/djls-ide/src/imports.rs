use djls_source::LineEnding;
use djls_source::Offset;
use djls_source::Span;
use djls_templates::Node;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LeadingImports {
    extends: Option<Span>,
    loads: Vec<Span>,
}

impl LeadingImports {
    pub(crate) fn load_insertion_offset(&self, source: &str) -> Offset {
        let Some(last_import_tag) = self.loads.last().copied().or(self.extends) else {
            return Offset::new(0);
        };

        offset_after_line(source, last_import_tag.end_usize())
    }
}

pub(crate) fn leading_imports(nodes: &[Node], source: &str) -> LeadingImports {
    let mut imports = LeadingImports::default();

    for node in nodes {
        match node {
            Node::Text { span }
                if source
                    .get(span.start_usize()..span.end_usize())
                    .is_some_and(|text| text.trim().is_empty()) => {}
            Node::Comment { .. } => {}
            Node::Tag { name, .. }
                if name == "extends" && imports.extends.is_none() && imports.loads.is_empty() =>
            {
                imports.extends = Some(node.full_span());
            }
            Node::Tag { name, .. } if name == "load" => {
                imports.loads.push(node.full_span());
            }
            Node::Tag { .. } | Node::Text { .. } | Node::Variable { .. } | Node::Error { .. } => {
                break;
            }
        }
    }

    imports
}

pub(crate) fn fold_spans(nodes: &[Node], source: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut import_run = ImportRun::None;

    for node in nodes {
        match node {
            Node::Tag { name, .. } if name == "extends" => {
                spans.extend(import_run.finish());
                import_run.start_at_extends(node.full_span());
            }
            Node::Tag { name, .. } if name == "load" => {
                import_run.include_load(node.full_span());
            }
            Node::Text { span }
                if source
                    .get(span.start_usize()..span.end_usize())
                    .is_some_and(|text| text.trim().is_empty()) => {}
            Node::Comment { .. }
            | Node::Tag { .. }
            | Node::Text { .. }
            | Node::Variable { .. }
            | Node::Error { .. } => {
                spans.extend(import_run.finish());
            }
        }
    }

    spans.extend(import_run.finish());
    spans
}

fn offset_after_line(source: &str, offset: usize) -> Offset {
    let bytes = source.as_bytes();
    let mut offset = offset.min(source.len());

    while offset < source.len() {
        if let Some(ending) = LineEnding::match_at(bytes, offset) {
            offset += ending.byte_len();
            break;
        }
        offset += 1;
    }

    Offset::try_from(offset).unwrap_or_else(|_| Offset::new(u32::MAX))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImportRun {
    None,
    ExtendsOnly { start: u32 },
    Imports { start: u32, end: u32 },
}

impl ImportRun {
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

    fn finish(&mut self) -> Option<Span> {
        match std::mem::replace(self, Self::None) {
            Self::Imports { start, end } if start < end => Some(Span::new(start, end - start)),
            Self::None | Self::ExtendsOnly { .. } | Self::Imports { .. } => None,
        }
    }
}

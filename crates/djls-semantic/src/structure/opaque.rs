use djls_source::Span;
use djls_templates::Node;
use djls_templates::NodeList;
use djls_templates::TagDelimiter;

use crate::db::Db;
use crate::structure::grammar::TagClass;
use crate::structure::grammar::compute_tag_index;

/// Sorted, non-overlapping byte-offset spans where validation should be skipped.
///
/// Represents the interior content of opaque blocks like `{% verbatim %}` and
/// `{% comment %}`, where the template engine treats the content as raw text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct OpaqueRegions {
    spans: Vec<Span>,
}

impl OpaqueRegions {
    #[must_use]
    pub fn new(mut spans: Vec<Span>) -> Self {
        spans.sort_by_key(|s| s.start());
        Self { spans }
    }

    /// Returns `true` if the given byte position falls inside an opaque region.
    #[must_use]
    pub fn is_opaque(&self, position: u32) -> bool {
        self.spans
            .binary_search_by(|span| {
                if position < span.start() {
                    std::cmp::Ordering::Greater
                } else if position >= span.end() {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .is_ok()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }
}

/// Compute opaque regions for a template by scanning for opaque block tags.
///
/// This uses a lightweight source-order scan instead of building a full
/// `TemplateTree`.
pub fn compute_opaque_regions(db: &dyn Db, nodelist: NodeList<'_>) -> OpaqueRegions {
    let tag_specs = db.tag_specs();
    let index = compute_tag_index(db);
    let mut spans = Vec::new();
    let mut stack = Vec::new();

    for node in nodelist.nodelist(db) {
        let Node::Tag { name, span, .. } = node else {
            continue;
        };

        match index.classify(name) {
            TagClass::Opener => {
                let body_start = span.end().saturating_add(TagDelimiter::LENGTH_U32);
                stack.push(OpaqueFrame {
                    opener_name: name,
                    segment_start: body_start,
                    is_opaque: tag_specs.get(name).is_some_and(|spec| spec.opaque),
                });
            }
            TagClass::Closer { opener_name } => {
                let Some(frame_idx) = stack
                    .iter()
                    .rposition(|frame| frame.opener_name == opener_name)
                else {
                    continue;
                };

                while stack.len() > frame_idx + 1 {
                    stack.pop();
                }

                let Some(frame) = stack.pop() else {
                    continue;
                };
                push_opaque_segment(&frame, *span, &mut spans);
            }
            TagClass::Intermediate { possible_openers } => {
                if let Some(frame) = stack.last_mut()
                    && possible_openers
                        .iter()
                        .any(|opener| opener == frame.opener_name)
                {
                    push_opaque_segment(frame, *span, &mut spans);
                    frame.segment_start = span.end().saturating_add(TagDelimiter::LENGTH_U32);
                }
            }
            TagClass::Unknown => {}
        }
    }

    OpaqueRegions::new(spans)
}

fn push_opaque_segment(frame: &OpaqueFrame<'_>, content_span: Span, spans: &mut Vec<Span>) {
    if frame.is_opaque {
        let content_end = content_span
            .start()
            .saturating_sub(TagDelimiter::LENGTH_U32);
        spans.push(Span::saturating_from_bounds_usize(
            frame.segment_start as usize,
            content_end as usize,
        ));
    }
}

struct OpaqueFrame<'a> {
    opener_name: &'a str,
    segment_start: u32,
    is_opaque: bool,
}

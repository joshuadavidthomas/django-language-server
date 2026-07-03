use djls_semantic::ValidationError;
use djls_semantic::ValidationErrorAccumulator;
use djls_source::File;
use djls_source::FileKind;
use djls_source::LineEnding;
use djls_source::Offset;
use djls_source::PositionEncoding;
use djls_source::Span;
use djls_templates::Node;
use tower_lsp_server::ls_types;

use crate::diagnostics::lsp_diagnostic_for;
use crate::ext::QuickFixActionExt;
use crate::ext::SpanExt;
use crate::ext::Utf8PathExt;
use crate::header::import_header;

#[must_use]
pub fn code_actions(
    db: &dyn djls_semantic::Db,
    file: File,
    range: Span,
    encoding: PositionEncoding,
) -> Option<Vec<ls_types::CodeActionOrCommand>> {
    let source = file.source(db);
    if *source.kind() != FileKind::Template {
        return None;
    }
    let source_text = source.as_str();

    let Some(parsed) = djls_templates::parse_template(db, file) else {
        return Some(Vec::new());
    };

    djls_semantic::validate_template_file(db, file);
    let errors =
        djls_semantic::validate_template_file::accumulated::<ValidationErrorAccumulator>(db, file);
    if errors.is_empty() {
        return Some(Vec::new());
    }

    let line_index = file.line_index(db);
    let config = db.diagnostics_config();
    let edit_context = EditContext {
        uri: file.path(db).to_lsp_uri()?,
        source: source_text,
        line_index,
        encoding,
    };
    let nodelist = parsed.nodelist(db);

    let mut actions = Vec::new();
    for error_acc in errors {
        let error = &error_acc.0;
        let Some(primary_span) = error.primary_span() else {
            continue;
        };
        let intersects_request = if range.length() == 0 {
            let offset = range.start_offset();
            primary_span.contains(offset) || offset.get() == primary_span.end()
        } else {
            primary_span.start() < range.end() && range.start() < primary_span.end()
        };
        if !intersects_request {
            continue;
        }

        match error {
            ValidationError::UnloadedTag { library, .. }
            | ValidationError::UnloadedFilter { library, .. } => {
                let Some(diagnostic) = lsp_diagnostic_for(error, line_index, &config) else {
                    continue;
                };
                let insertion_offset =
                    import_header(nodelist, source_text).load_insertion_offset(source_text);
                actions.push(insert_load_action(
                    &edit_context,
                    insertion_offset,
                    diagnostic,
                    library,
                    Some(true),
                ));
            }
            ValidationError::AmbiguousUnloadedTag { libraries, .. }
            | ValidationError::AmbiguousUnloadedFilter { libraries, .. } => {
                let Some(diagnostic) = lsp_diagnostic_for(error, line_index, &config) else {
                    continue;
                };
                let insertion_offset =
                    import_header(nodelist, source_text).load_insertion_offset(source_text);
                let mut libraries = libraries.iter().map(String::as_str).collect::<Vec<_>>();
                libraries.sort_unstable();
                libraries.dedup();
                for library in libraries {
                    actions.push(insert_load_action(
                        &edit_context,
                        insertion_offset,
                        diagnostic.clone(),
                        library,
                        None,
                    ));
                }
            }
            ValidationError::UnmatchedBlockName { expected, span, .. } => {
                let Some(name_span) = closing_block_name_span(nodelist, *span) else {
                    continue;
                };
                let Some(diagnostic) = lsp_diagnostic_for(error, line_index, &config) else {
                    continue;
                };
                actions.push(rename_closing_block_action(
                    &edit_context,
                    diagnostic,
                    name_span,
                    expected,
                ));
            }
            _ => {}
        }
    }

    Some(actions)
}

struct EditContext<'a> {
    uri: ls_types::Uri,
    source: &'a str,
    line_index: &'a djls_source::LineIndex,
    encoding: PositionEncoding,
}

fn insert_load_action(
    context: &EditContext<'_>,
    insertion_offset: Offset,
    diagnostic: ls_types::Diagnostic,
    library: &str,
    is_preferred: Option<bool>,
) -> ls_types::CodeActionOrCommand {
    let line_ending = LineEnding::last_in(context.source)
        .unwrap_or_default()
        .as_str();
    let load_line = format!("{{% load {library} %}}{line_ending}");
    let offset = insertion_offset.get() as usize;
    let new_text = if !context.source.is_empty()
        && offset == context.source.len()
        && !context.source.ends_with('\n')
        && !context.source.ends_with('\r')
    {
        format!("{line_ending}{load_line}")
    } else {
        load_line
    };

    let edit = ls_types::TextEdit::new(
        Span::new(insertion_offset.get(), 0).to_lsp_range_with_encoding(
            context.source,
            context.line_index,
            context.encoding,
        ),
        new_text,
    );

    vec![edit].to_quick_fix_action(
        context.uri.clone(),
        format!("Add '{{% load {library} %}}'"),
        diagnostic,
        is_preferred,
    )
}

fn rename_closing_block_action(
    context: &EditContext<'_>,
    diagnostic: ls_types::Diagnostic,
    name_span: Span,
    expected: &str,
) -> ls_types::CodeActionOrCommand {
    let edit = ls_types::TextEdit::new(
        name_span.to_lsp_range_with_encoding(context.source, context.line_index, context.encoding),
        expected.to_string(),
    );

    vec![edit].to_quick_fix_action(
        context.uri.clone(),
        format!("Rename closing block to '{expected}'"),
        diagnostic,
        Some(true),
    )
}

fn closing_block_name_span(nodelist: &[Node], full_span: Span) -> Option<Span> {
    nodelist.iter().find_map(|node| match node {
        Node::Tag { bits, .. } if node.full_span() == full_span => bits.first().map(|bit| bit.span),
        Node::Tag { .. }
        | Node::Comment { .. }
        | Node::Text { .. }
        | Node::Variable { .. }
        | Node::Error { .. } => None,
    })
}

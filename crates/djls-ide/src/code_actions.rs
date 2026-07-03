use djls_semantic::ValidationError;
use djls_semantic::ValidationErrorAccumulator;
use djls_source::File;
use djls_source::FileKind;
use djls_source::LineEnding;
use djls_source::LineIndex;
use djls_source::Offset;
use djls_source::PositionEncoding;
use djls_source::Span;
use tower_lsp_server::ls_types;

use crate::ext::DiagnosticExt;
use crate::ext::QuickFixActionExt;
use crate::ext::SpanExt;
use crate::ext::Utf8PathExt;
use crate::imports::leading_imports;

#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "the exhaustive ValidationError dispatch is clearer kept in one place"
)]
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
    let uri = file.path(db).to_lsp_uri()?;
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
                let Some(diagnostic) = error.to_lsp_diagnostic(line_index, &config) else {
                    continue;
                };
                let insertion_offset =
                    leading_imports(nodelist, source_text).load_insertion_offset(source_text);
                let edit =
                    load_tag_edit(source_text, line_index, encoding, insertion_offset, library);
                actions.push(vec![edit].to_quick_fix_action(
                    uri.clone(),
                    format!("Add '{{% load {library} %}}'"),
                    diagnostic,
                    Some(true),
                ));
            }
            ValidationError::AmbiguousUnloadedTag { libraries, .. }
            | ValidationError::AmbiguousUnloadedFilter { libraries, .. } => {
                let Some(diagnostic) = error.to_lsp_diagnostic(line_index, &config) else {
                    continue;
                };
                let insertion_offset =
                    leading_imports(nodelist, source_text).load_insertion_offset(source_text);
                let mut libraries = libraries.iter().map(String::as_str).collect::<Vec<_>>();
                libraries.sort_unstable();
                libraries.dedup();

                for library in libraries {
                    let edit =
                        load_tag_edit(source_text, line_index, encoding, insertion_offset, library);
                    actions.push(vec![edit].to_quick_fix_action(
                        uri.clone(),
                        format!("Add '{{% load {library} %}}'"),
                        diagnostic.clone(),
                        None,
                    ));
                }
            }
            ValidationError::UnmatchedBlockName {
                expected, got_span, ..
            } => {
                let Some(diagnostic) = error.to_lsp_diagnostic(line_index, &config) else {
                    continue;
                };
                let edit = ls_types::TextEdit::new(
                    got_span.to_lsp_range_with_encoding(source_text, line_index, encoding),
                    expected.clone(),
                );
                actions.push(vec![edit].to_quick_fix_action(
                    uri.clone(),
                    format!("Rename closing block to '{expected}'"),
                    diagnostic,
                    Some(true),
                ));
            }
            ValidationError::UnclosedTag { .. }
            | ValidationError::OrphanedTag { .. }
            | ValidationError::OrphanedClosingTag { .. }
            | ValidationError::UnbalancedStructure { .. }
            | ValidationError::UnknownTag { .. }
            | ValidationError::TagNotInInstalledApps { .. }
            | ValidationError::UnknownFilter { .. }
            | ValidationError::FilterNotInInstalledApps { .. }
            | ValidationError::ExpressionSyntaxError { .. }
            | ValidationError::FilterMissingArgument { .. }
            | ValidationError::FilterUnexpectedArgument { .. }
            | ValidationError::ExtractedRuleViolation { .. }
            | ValidationError::UnknownLibrary { .. }
            | ValidationError::LibraryNotInInstalledApps { .. }
            | ValidationError::ExtendsMustBeFirst { .. }
            | ValidationError::MultipleExtends { .. } => {}
        }
    }

    Some(actions)
}

fn load_tag_edit(
    source_text: &str,
    line_index: &LineIndex,
    encoding: PositionEncoding,
    insertion_offset: Offset,
    library: &str,
) -> ls_types::TextEdit {
    let line_ending = LineEnding::last_in(source_text)
        .unwrap_or_default()
        .as_str();
    let load_line = format!("{{% load {library} %}}{line_ending}");
    let offset = insertion_offset.get() as usize;
    let new_text = if !source_text.is_empty()
        && offset == source_text.len()
        && !source_text.ends_with('\n')
        && !source_text.ends_with('\r')
    {
        format!("{line_ending}{load_line}")
    } else {
        load_line
    };

    ls_types::TextEdit::new(
        Span::new(insertion_offset.get(), 0).to_lsp_range_with_encoding(
            source_text,
            line_index,
            encoding,
        ),
        new_text,
    )
}

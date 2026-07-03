use std::collections::HashMap;

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

    let Some(parsed) = djls_templates::parse_template(db, file) else {
        return Some(Vec::new());
    };

    djls_semantic::validate_template_file(db, file);
    let errors =
        djls_semantic::validate_template_file::accumulated::<ValidationErrorAccumulator>(db, file);
    let actionable_errors = errors
        .iter()
        .map(|error_acc| &error_acc.0)
        .filter(|error| is_actionable_for_range(error, range))
        .collect::<Vec<_>>();
    if actionable_errors.is_empty() {
        return Some(Vec::new());
    }

    let context = CodeActionContext {
        uri: file.path(db).to_lsp_uri()?,
        source: source.as_str(),
        line_index: file.line_index(db),
        diagnostics_config: db.diagnostics_config(),
        encoding,
    };
    let nodelist = parsed.nodelist(db);

    let mut actions = Vec::new();
    for error in actionable_errors {
        add_actions_for_error(&mut actions, error, nodelist, &context);
    }

    Some(actions)
}

struct CodeActionContext<'a> {
    uri: ls_types::Uri,
    source: &'a str,
    line_index: &'a djls_source::LineIndex,
    diagnostics_config: djls_conf::DiagnosticsConfig,
    encoding: PositionEncoding,
}

fn add_actions_for_error(
    actions: &mut Vec<ls_types::CodeActionOrCommand>,
    error: &ValidationError,
    nodelist: &[Node],
    context: &CodeActionContext<'_>,
) {
    match error {
        ValidationError::UnloadedTag { library, .. }
        | ValidationError::UnloadedFilter { library, .. } => {
            let Some(diagnostic) = diagnostic_for_action(error, context) else {
                return;
            };
            actions.push(insert_load_action(
                context,
                load_insertion_offset(nodelist, context.source),
                diagnostic,
                library,
                Some(true),
            ));
        }
        ValidationError::AmbiguousUnloadedTag { libraries, .. }
        | ValidationError::AmbiguousUnloadedFilter { libraries, .. } => {
            let Some(diagnostic) = diagnostic_for_action(error, context) else {
                return;
            };
            let insertion_offset = load_insertion_offset(nodelist, context.source);
            for library in sorted_libraries(libraries) {
                actions.push(insert_load_action(
                    context,
                    insertion_offset,
                    diagnostic.clone(),
                    &library,
                    None,
                ));
            }
        }
        ValidationError::UnmatchedBlockName { expected, span, .. } => {
            let Some(name_span) = closing_block_name_span(nodelist, *span) else {
                return;
            };
            let Some(diagnostic) = diagnostic_for_action(error, context) else {
                return;
            };
            actions.push(rename_closing_block_action(
                context, diagnostic, name_span, expected,
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

fn is_actionable_for_range(error: &ValidationError, request_range: Span) -> bool {
    is_actionable(error)
        && error
            .primary_span()
            .is_some_and(|span| span_intersects_request(span, request_range))
}

fn is_actionable(error: &ValidationError) -> bool {
    match error {
        ValidationError::UnloadedTag { .. }
        | ValidationError::UnloadedFilter { .. }
        | ValidationError::AmbiguousUnloadedTag { .. }
        | ValidationError::AmbiguousUnloadedFilter { .. }
        | ValidationError::UnmatchedBlockName { .. } => true,
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
        | ValidationError::MultipleExtends { .. } => false,
    }
}

fn diagnostic_for_action(
    error: &ValidationError,
    context: &CodeActionContext<'_>,
) -> Option<ls_types::Diagnostic> {
    lsp_diagnostic_for(error, context.line_index, &context.diagnostics_config)
}

fn load_insertion_offset(nodelist: &[Node], source: &str) -> Offset {
    import_header(nodelist, source).load_insertion_offset(source)
}

fn insert_load_action(
    context: &CodeActionContext<'_>,
    insertion_offset: Offset,
    diagnostic: ls_types::Diagnostic,
    library: &str,
    is_preferred: Option<bool>,
) -> ls_types::CodeActionOrCommand {
    let edit = ls_types::TextEdit::new(
        Span::new(insertion_offset.get(), 0).to_lsp_range_with_encoding(
            context.source,
            context.line_index,
            context.encoding,
        ),
        load_edit_text(context.source, insertion_offset, library),
    );

    quick_fix_action(
        context,
        format!("Add '{{% load {library} %}}'"),
        diagnostic,
        vec![edit],
        is_preferred,
    )
}

fn rename_closing_block_action(
    context: &CodeActionContext<'_>,
    diagnostic: ls_types::Diagnostic,
    name_span: Span,
    expected: &str,
) -> ls_types::CodeActionOrCommand {
    let edit = ls_types::TextEdit::new(
        name_span.to_lsp_range_with_encoding(context.source, context.line_index, context.encoding),
        expected.to_string(),
    );

    quick_fix_action(
        context,
        format!("Rename closing block to '{expected}'"),
        diagnostic,
        vec![edit],
        Some(true),
    )
}

fn quick_fix_action(
    context: &CodeActionContext<'_>,
    title: String,
    diagnostic: ls_types::Diagnostic,
    edits: Vec<ls_types::TextEdit>,
    is_preferred: Option<bool>,
) -> ls_types::CodeActionOrCommand {
    let workspace_edit = ls_types::WorkspaceEdit {
        changes: Some(HashMap::from([(context.uri.clone(), edits)])),
        document_changes: None,
        change_annotations: None,
    };

    ls_types::CodeActionOrCommand::CodeAction(ls_types::CodeAction {
        title,
        kind: Some(ls_types::CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diagnostic]),
        edit: Some(workspace_edit),
        command: None,
        is_preferred,
        disabled: None,
        data: None,
    })
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

fn sorted_libraries(libraries: &[String]) -> Vec<String> {
    let mut libraries = libraries.to_vec();
    libraries.sort_unstable();
    libraries.dedup();
    libraries
}

fn load_edit_text(source: &str, insertion_offset: Offset, library: &str) -> String {
    let line_ending = LineEnding::last_in(source).unwrap_or_default().as_str();
    let load_line = format!("{{% load {library} %}}{line_ending}");

    let insertion_offset = insertion_offset.get() as usize;
    if !source.is_empty()
        && insertion_offset == source.len()
        && !source.ends_with('\n')
        && !source.ends_with('\r')
    {
        format!("{line_ending}{load_line}")
    } else {
        load_line
    }
}

fn span_intersects_request(span: Span, request: Span) -> bool {
    if request.length() == 0 {
        let offset = request.start_offset();
        return span.contains(offset) || offset.get() == span.end();
    }

    span.start() < request.end() && request.start() < span.end()
}

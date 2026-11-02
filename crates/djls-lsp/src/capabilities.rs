use djls_ide::REPORT_UNREADABLE_REGISTRATION_COMMAND;
use djls_source::PositionEncoding;
use tower_lsp_server::ls_types;

use crate::ext::PositionEncodingExt;

/// Everything the native server advertises. Servers that implement a subset pick
/// their providers from this rather than restating them.
#[must_use]
pub fn server_capabilities(encoding: PositionEncoding) -> ls_types::ServerCapabilities {
    ls_types::ServerCapabilities {
        completion_provider: Some(ls_types::CompletionOptions {
            resolve_provider: Some(false),
            trigger_characters: Some(vec![
                "{".to_string(),
                "%".to_string(),
                " ".to_string(),
                "\"".to_string(),
                "'".to_string(),
            ]),
            ..Default::default()
        }),
        workspace: Some(ls_types::WorkspaceServerCapabilities {
            workspace_folders: Some(ls_types::WorkspaceFoldersServerCapabilities {
                supported: Some(true),
                change_notifications: Some(ls_types::OneOf::Left(true)),
            }),
            file_operations: None,
        }),
        text_document_sync: Some(ls_types::TextDocumentSyncCapability::Options(
            ls_types::TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(ls_types::TextDocumentSyncKind::INCREMENTAL),
                will_save: Some(false),
                will_save_wait_until: Some(false),
                save: Some(ls_types::SaveOptions::default().into()),
            },
        )),
        position_encoding: Some(encoding.to_lsp()),
        diagnostic_provider: Some(ls_types::DiagnosticServerCapabilities::Options(
            ls_types::DiagnosticOptions {
                identifier: None,
                inter_file_dependencies: false,
                workspace_diagnostics: false,
                work_done_progress_options: ls_types::WorkDoneProgressOptions::default(),
            },
        )),
        code_action_provider: Some(ls_types::CodeActionProviderCapability::Options(
            ls_types::CodeActionOptions {
                code_action_kinds: Some(vec![ls_types::CodeActionKind::QUICKFIX]),
                work_done_progress_options: ls_types::WorkDoneProgressOptions::default(),
                resolve_provider: Some(false),
            },
        )),
        execute_command_provider: Some(ls_types::ExecuteCommandOptions {
            commands: vec![REPORT_UNREADABLE_REGISTRATION_COMMAND.to_string()],
            work_done_progress_options: ls_types::WorkDoneProgressOptions::default(),
        }),
        folding_range_provider: Some(ls_types::FoldingRangeProviderCapability::Simple(true)),
        document_symbol_provider: Some(ls_types::OneOf::Left(true)),
        document_link_provider: Some(ls_types::DocumentLinkOptions {
            resolve_provider: Some(false),
            work_done_progress_options: ls_types::WorkDoneProgressOptions::default(),
        }),
        hover_provider: Some(ls_types::HoverProviderCapability::Simple(true)),
        definition_provider: Some(ls_types::OneOf::Left(true)),
        references_provider: Some(ls_types::OneOf::Left(true)),
        document_formatting_provider: Some(ls_types::OneOf::Left(true)),
        ..Default::default()
    }
}

#[must_use]
pub fn server_info() -> ls_types::ServerInfo {
    ls_types::ServerInfo {
        name: "Django Language Server".to_string(),
        version: Some(crate::VERSION.to_string()),
    }
}

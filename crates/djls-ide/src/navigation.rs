use djls_semantic::resolve_template;
use djls_semantic::ResolveResult;
use djls_source::File;
use djls_source::Offset;
use tower_lsp_server::lsp_types;

use crate::context::OffsetContext;
use crate::ext::SpanExt;
use crate::ext::Utf8PathExt;

pub fn goto_definition(
    db: &dyn djls_semantic::Db,
    file: File,
    offset: Offset,
) -> Option<lsp_types::GotoDefinitionResponse> {
    match OffsetContext::from_offset(db, file, offset) {
        OffsetContext::TemplateReference(template_name) => {
            tracing::debug!("Found template reference: '{}'", template_name);

            match resolve_template(db, &template_name) {
                ResolveResult::Found(template) => {
                    let path = template.path_buf(db);
                    tracing::debug!("Resolved template to: {}", path);

                    Some(lsp_types::GotoDefinitionResponse::Scalar(
                        lsp_types::Location {
                            uri: path.to_lsp_uri()?,
                            range: lsp_types::Range::default(),
                        },
                    ))
                }
                ResolveResult::NotFound { tried, .. } => {
                    tracing::warn!("Template '{}' not found. Tried: {:?}", template_name, tried);
                    None
                }
            }
        }
        OffsetContext::None => None,
    }
}

pub fn find_references(
    db: &dyn djls_semantic::Db,
    file: File,
    offset: Offset,
) -> Option<Vec<lsp_types::Location>> {
    match OffsetContext::from_offset(db, file, offset) {
        OffsetContext::TemplateReference(template_name) => {
            tracing::debug!(
                "Cursor is inside extends/include tag referencing: '{}'",
                template_name
            );

            let references = djls_semantic::find_references_to_template(db, &template_name);

            let locations: Vec<lsp_types::Location> = references
                .iter()
                .filter_map(|reference| {
                    let ref_file = reference.source_file(db);
                    let line_index = ref_file.line_index(db);

                    Some(lsp_types::Location {
                        uri: ref_file.path(db).to_lsp_uri()?,
                        range: reference.tag_span(db).to_lsp_range(line_index),
                    })
                })
                .collect();

            if locations.is_empty() {
                None
            } else {
                Some(locations)
            }
        }
        OffsetContext::None => None,
    }
}

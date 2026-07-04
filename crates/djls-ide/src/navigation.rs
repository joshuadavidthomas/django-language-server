use djls_project::FindTemplateResult;
use djls_project::template_resolution;
use djls_semantic::SemanticOffsetContext;
use djls_semantic::references_to_template_name;
use djls_semantic::resolve_reference_name;
use djls_source::File;
use djls_source::Offset;
use tower_lsp_server::ls_types;

use crate::ext::SpanExt;
use crate::ext::Utf8PathExt;

pub fn goto_definition(
    db: &dyn djls_semantic::Db,
    file: File,
    offset: Offset,
    supports_location_links: bool,
) -> Option<ls_types::GotoDefinitionResponse> {
    match SemanticOffsetContext::from_offset(db, file, offset) {
        SemanticOffsetContext::TemplateReference {
            name: template_name,
            kind,
            span,
        } => {
            tracing::debug!("Found template reference: '{}'", template_name.name(db));

            let project = db.project()?;
            let resolution = template_resolution(db, project);
            let template_name = resolve_reference_name(db, resolution, file, template_name, kind)?;

            match resolution.resolve(db, template_name) {
                FindTemplateResult::Found(origin) => {
                    let path = origin.path_buf(db);
                    tracing::debug!("Resolved template to: {}", path);

                    let target_uri = path.to_lsp_uri()?;
                    let target_range = ls_types::Range::default();
                    if supports_location_links {
                        Some(ls_types::GotoDefinitionResponse::Link(vec![
                            ls_types::LocationLink {
                                origin_selection_range: Some(
                                    span.to_lsp_range(file.line_index(db)),
                                ),
                                target_uri,
                                target_range,
                                target_selection_range: target_range,
                            },
                        ]))
                    } else {
                        Some(ls_types::GotoDefinitionResponse::Scalar(
                            ls_types::Location {
                                uri: target_uri,
                                range: target_range,
                            },
                        ))
                    }
                }
                FindTemplateResult::DoesNotExist(error) => {
                    tracing::warn!(
                        "Template '{}' not found. Tried: {:?}",
                        error.template_name.name(db),
                        error.tried
                    );
                    None
                }
            }
        }
        _ => None,
    }
}

pub fn find_references(
    db: &dyn djls_semantic::Db,
    file: File,
    offset: Offset,
) -> Option<Vec<ls_types::Location>> {
    match SemanticOffsetContext::from_offset(db, file, offset) {
        SemanticOffsetContext::TemplateReference {
            name: template_name,
            kind,
            ..
        } => {
            tracing::debug!(
                "Cursor is inside template-reference tag referencing: '{}'",
                template_name.name(db)
            );

            let project = db.project()?;
            let resolution = template_resolution(db, project);
            let template_name = resolve_reference_name(db, resolution, file, template_name, kind)?;
            let references = references_to_template_name(db, project, template_name);

            let locations: Vec<ls_types::Location> = references
                .iter()
                .filter_map(|reference| {
                    let ref_file = reference.source_file(db);
                    let line_index = ref_file.line_index(db);

                    Some(ls_types::Location {
                        uri: ref_file.path(db).to_lsp_uri()?,
                        range: reference.span(db).to_lsp_range(line_index),
                    })
                })
                .collect();

            if locations.is_empty() {
                None
            } else {
                Some(locations)
            }
        }
        _ => None,
    }
}

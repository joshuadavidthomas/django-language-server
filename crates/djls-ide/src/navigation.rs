use djls_semantic::FindTemplateResult;
use djls_semantic::SemanticOffsetContext;
use djls_semantic::find_template;
use djls_semantic::references_to_template_name;
use djls_source::File;
use djls_source::Offset;
use tower_lsp_server::ls_types;

use crate::ext::SpanExt;
use crate::ext::Utf8PathExt;

pub fn goto_definition(
    db: &dyn djls_semantic::Db,
    file: File,
    offset: Offset,
) -> Option<ls_types::GotoDefinitionResponse> {
    match SemanticOffsetContext::from_offset(db, file, offset) {
        SemanticOffsetContext::TemplateReference {
            name: template_name,
            ..
        } => {
            tracing::debug!("Found template reference: '{}'", template_name.name(db));

            let project = db.project()?;

            match find_template(db, project, template_name) {
                FindTemplateResult::Found(origin) => {
                    let path = origin.path_buf(db);
                    tracing::debug!("Resolved template to: {}", path);

                    Some(ls_types::GotoDefinitionResponse::Scalar(
                        ls_types::Location {
                            uri: path.to_lsp_uri()?,
                            range: ls_types::Range::default(),
                        },
                    ))
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
            ..
        } => {
            tracing::debug!(
                "Cursor is inside template-reference tag referencing: '{}'",
                template_name.name(db)
            );

            let project = db.project()?;
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

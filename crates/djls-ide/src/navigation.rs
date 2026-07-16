use djls_project::FindTemplateResult;
use djls_project::template_resolution;
use djls_semantic::SemanticOffsetContext;
use djls_semantic::references_to_template_name;
use djls_semantic::resolve_reference_for_file;
use djls_semantic::resolve_reference_origins;
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
            match resolve_reference_for_file(db, resolution, file, template_name, kind)? {
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
                FindTemplateResult::Inconclusive(search) => {
                    // Jumping to a probable origin beats refusing to navigate; with several
                    // candidates the editor presents the list and the user picks.
                    if supports_location_links {
                        let links = search
                            .possible_origins
                            .iter()
                            .filter_map(|origin| {
                                let target_uri = origin.path_buf(db).to_lsp_uri()?;
                                let target_range = ls_types::Range::default();
                                Some(ls_types::LocationLink {
                                    origin_selection_range: Some(
                                        span.to_lsp_range(file.line_index(db)),
                                    ),
                                    target_uri,
                                    target_range,
                                    target_selection_range: target_range,
                                })
                            })
                            .collect::<Vec<_>>();
                        (!links.is_empty()).then_some(ls_types::GotoDefinitionResponse::Link(links))
                    } else {
                        let locations = search
                            .possible_origins
                            .iter()
                            .filter_map(|origin| {
                                Some(ls_types::Location {
                                    uri: origin.path_buf(db).to_lsp_uri()?,
                                    range: ls_types::Range::default(),
                                })
                            })
                            .collect::<Vec<_>>();
                        (!locations.is_empty())
                            .then_some(ls_types::GotoDefinitionResponse::Array(locations))
                    }
                }
                FindTemplateResult::DoesNotExist(error) => {
                    tracing::warn!(
                        "Template '{}' not found. Tried: {:?}",
                        error.name.name(db),
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
            let FindTemplateResult::Found(target_origin) =
                resolve_reference_for_file(db, resolution, file, template_name, kind)?
            else {
                return None;
            };
            let origin_outcomes =
                resolve_reference_origins(db, resolution, file, template_name, kind);
            let target_names = if origin_outcomes.is_empty() {
                // A successful originless resolution can only be for an absolute name, which is
                // already normalized and can be used directly for reverse lookup.
                vec![template_name]
            } else {
                origin_outcomes
                    .into_iter()
                    .map(|outcome| outcome.target_name)
                    .collect()
            };

            let mut locations: Vec<ls_types::Location> = Vec::new();
            for target_name in target_names {
                for reference in references_to_template_name(db, project, target_name) {
                    let ref_file = reference.source_file(db);
                    let Some(outcome) = reference.resolve(db, resolution) else {
                        continue;
                    };
                    if !matches!(
                        outcome.result,
                        FindTemplateResult::Found(origin) if origin.file(db) == target_origin.file(db)
                    ) {
                        continue;
                    }
                    let Some(uri) = ref_file.path(db).to_lsp_uri() else {
                        continue;
                    };
                    let location = ls_types::Location {
                        uri,
                        range: reference.span(db).to_lsp_range(ref_file.line_index(db)),
                    };
                    if !locations.contains(&location) {
                        locations.push(location);
                    }
                }
            }

            if locations.is_empty() {
                None
            } else {
                Some(locations)
            }
        }
        _ => None,
    }
}

use djls_project::LoadableLibraryLookup;
use djls_project::TemplateName;
use djls_project::TemplateResolutionResult;
use djls_project::TemplateSymbolKind;
use djls_project::TemplateSymbolSource;
use djls_project::template_resolution;
use djls_project::template_symbol_source;
use djls_semantic::SemanticOffsetContext;
use djls_semantic::TemplateReferenceKind;
use djls_semantic::effective_symbol_candidate_at;
use djls_semantic::references_to_template_name;
use djls_semantic::resolve_reference_for_file;
use djls_semantic::resolve_reference_origins;
use djls_semantic::scoped_template_libraries_for_file;
use djls_source::File;
use djls_source::Offset;
use djls_source::PositionEncoding;
use djls_source::Span;
use djls_templates::TemplateParseResult;
use djls_templates::parse_template;
use tower_lsp_server::ls_types;

use crate::ext::DefinitionTargetExt;
use crate::ext::SpanExt;
use crate::ext::Utf8PathExt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DefinitionTarget {
    File(File),
    Symbol(TemplateSymbolSource),
}

fn encoded_range(
    db: &dyn djls_semantic::Db,
    file: File,
    span: Span,
    position_encoding: PositionEncoding,
) -> Option<ls_types::Range> {
    let source = file.try_source(db).ok()?;
    Some(span.to_lsp_range_with_encoding(source.as_str(), file.line_index(db), position_encoding))
}

fn exact_definition_response(
    db: &dyn djls_semantic::Db,
    origin_selection_range: ls_types::Range,
    targets: Vec<DefinitionTarget>,
    supports_location_links: bool,
    position_encoding: PositionEncoding,
) -> Option<ls_types::GotoDefinitionResponse> {
    let targets = targets
        .into_iter()
        .filter_map(|target| target.to_lsp_parts(db, position_encoding))
        .collect::<Vec<_>>();
    if targets.is_empty() {
        return None;
    }
    if supports_location_links {
        return Some(ls_types::GotoDefinitionResponse::Link(
            targets
                .into_iter()
                .map(
                    |(target_uri, target_range, target_selection_range)| ls_types::LocationLink {
                        origin_selection_range: Some(origin_selection_range),
                        target_uri,
                        target_range,
                        target_selection_range,
                    },
                )
                .collect(),
        ));
    }

    let mut locations = targets
        .into_iter()
        .map(|(uri, range, _selection_range)| ls_types::Location { uri, range })
        .collect::<Vec<_>>();
    if locations.len() == 1 {
        Some(ls_types::GotoDefinitionResponse::Scalar(locations.pop()?))
    } else {
        Some(ls_types::GotoDefinitionResponse::Array(locations))
    }
}

fn symbol_definition_target(
    db: &dyn djls_semantic::Db,
    symbol: &djls_project::TemplateSymbol,
) -> Option<DefinitionTarget> {
    Some(DefinitionTarget::Symbol(template_symbol_source(
        db, symbol,
    )?))
}

fn symbol_occurrence_response(
    db: &dyn djls_semantic::Db,
    file: File,
    name: &str,
    span: Span,
    kind: TemplateSymbolKind,
    supports_location_links: bool,
    position_encoding: PositionEncoding,
) -> Option<ls_types::GotoDefinitionResponse> {
    let TemplateParseResult::Parsed(nodelist) = parse_template(db, file) else {
        return None;
    };
    let candidate = effective_symbol_candidate_at(db, file, nodelist, span.start(), name, kind)?;
    exact_definition_response(
        db,
        encoded_range(db, file, span, position_encoding)?,
        vec![symbol_definition_target(db, &candidate.symbol)?],
        supports_location_links,
        position_encoding,
    )
}

fn template_reference_response(
    db: &dyn djls_semantic::Db,
    file: File,
    template_name: TemplateName<'_>,
    kind: TemplateReferenceKind,
    span: Span,
    supports_location_links: bool,
    position_encoding: PositionEncoding,
) -> Option<ls_types::GotoDefinitionResponse> {
    tracing::debug!("Found template reference: '{}'", template_name.name(db));

    let origin_selection_range = encoded_range(db, file, span, position_encoding)?;
    let project = db.project()?;
    let resolution = template_resolution(db, project);
    match resolve_reference_for_file(db, resolution, file, template_name, kind)? {
        TemplateResolutionResult::Found(origin) => {
            let path = origin.path_buf(db);
            tracing::debug!("Resolved template to: {}", path);

            let target_uri = path.to_lsp_uri()?;
            let target_range = ls_types::Range::default();
            if supports_location_links {
                Some(ls_types::GotoDefinitionResponse::Link(vec![
                    ls_types::LocationLink {
                        origin_selection_range: Some(origin_selection_range),
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
        TemplateResolutionResult::Inconclusive(search) => {
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
                            origin_selection_range: Some(origin_selection_range),
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
        TemplateResolutionResult::DoesNotExist(error) => {
            tracing::warn!(
                "Template '{}' not found. Tried: {:?}",
                error.name.name(db),
                error.tried
            );
            None
        }
    }
}

pub fn goto_definition(
    db: &dyn djls_semantic::Db,
    file: File,
    offset: Offset,
    supports_location_links: bool,
    position_encoding: PositionEncoding,
) -> Option<ls_types::GotoDefinitionResponse> {
    match SemanticOffsetContext::from_offset(db, file, offset) {
        SemanticOffsetContext::TemplateReference {
            name: template_name,
            kind,
            span,
        } => template_reference_response(
            db,
            file,
            template_name,
            kind,
            span,
            supports_location_links,
            position_encoding,
        ),
        SemanticOffsetContext::LoadLibrary { name, span } => {
            let scoped_libraries = scoped_template_libraries_for_file(db, file);
            let LoadableLibraryLookup::Found(library) =
                scoped_libraries.loadable_library_str(&name)
            else {
                return None;
            };
            exact_definition_response(
                db,
                encoded_range(db, file, span, position_encoding)?,
                vec![DefinitionTarget::File(library.source_file()?)],
                supports_location_links,
                position_encoding,
            )
        }
        SemanticOffsetContext::LoadSymbol {
            name,
            library,
            span,
        } => {
            let scoped_libraries = scoped_template_libraries_for_file(db, file);
            let LoadableLibraryLookup::Found(library) =
                scoped_libraries.loadable_library_str(&library)
            else {
                return None;
            };
            let mut targets = [TemplateSymbolKind::Tag, TemplateSymbolKind::Filter]
                .into_iter()
                .filter_map(|kind| library.symbol(kind, &name))
                .filter_map(|symbol| symbol_definition_target(db, symbol))
                .collect::<Vec<_>>();
            targets.dedup();
            exact_definition_response(
                db,
                encoded_range(db, file, span, position_encoding)?,
                targets,
                supports_location_links,
                position_encoding,
            )
        }
        SemanticOffsetContext::Tag { name, span, .. } => symbol_occurrence_response(
            db,
            file,
            &name,
            span,
            TemplateSymbolKind::Tag,
            supports_location_links,
            position_encoding,
        ),
        SemanticOffsetContext::Filter { name, span, .. } => symbol_occurrence_response(
            db,
            file,
            &name,
            span,
            TemplateSymbolKind::Filter,
            supports_location_links,
            position_encoding,
        ),
        SemanticOffsetContext::Variable { .. } | SemanticOffsetContext::None => None,
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
            let TemplateResolutionResult::Found(target_origin) =
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
                        TemplateResolutionResult::Found(origin) if origin.file(db) == target_origin.file(db)
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
        SemanticOffsetContext::LoadLibrary { .. }
        | SemanticOffsetContext::LoadSymbol { .. }
        | SemanticOffsetContext::Tag { .. }
        | SemanticOffsetContext::Filter { .. }
        | SemanticOffsetContext::Variable { .. }
        | SemanticOffsetContext::None => None,
    }
}

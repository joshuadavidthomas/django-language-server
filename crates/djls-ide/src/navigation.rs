use djls_semantic::resolve_template;
use djls_semantic::ResolveResult;
use djls_source::File;
use djls_source::LineCol;
use djls_source::Offset;
use djls_source::PositionEncoding;
use djls_templates::parse_template;
use djls_templates::Node;
use tower_lsp_server::lsp_types;
use tower_lsp_server::UriExt;

pub fn goto_template_definition(
    db: &dyn djls_semantic::Db,
    file: File,
    position: lsp_types::Position,
    encoding: PositionEncoding,
) -> Option<lsp_types::GotoDefinitionResponse> {
    let nodelist = parse_template(db, file)?;

    let line_index = file.line_index(db);
    let source = file.source(db);
    let line_col = LineCol::new(position.line, position.character);

    let offset = encoding.line_col_to_offset(line_index, line_col, source.as_str())?;

    let template_name = find_template_name_at_offset(nodelist.nodelist(db), offset)?;
    tracing::debug!("Found template reference: '{}'", template_name);

    match resolve_template(db, &template_name) {
        ResolveResult::Found(template) => {
            let path = template.path_buf(db);
            tracing::debug!("Resolved template to: {}", path);
            let uri = lsp_types::Uri::from_file_path(path.as_std_path())?;

            Some(lsp_types::GotoDefinitionResponse::Scalar(
                lsp_types::Location {
                    uri,
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

pub fn find_template_references(
    db: &dyn djls_semantic::Db,
    file: File,
    position: lsp_types::Position,
    encoding: PositionEncoding,
) -> Option<Vec<lsp_types::Location>> {
    let nodelist = parse_template(db, file)?;
    let line_index = file.line_index(db);
    let source = file.source(db);
    let line_col = LineCol::new(position.line, position.character);
    let offset = encoding.line_col_to_offset(line_index, line_col, source.as_str())?;

    let template_name = find_template_name_at_offset(nodelist.nodelist(db), offset)?;
    tracing::debug!(
        "Cursor is inside extends/include tag referencing: '{}'",
        template_name
    );

    let references = djls_semantic::find_references_to_template(db, &template_name);

    let locations: Vec<lsp_types::Location> = references
        .iter()
        .filter_map(|reference| {
            let source_template = reference.source(db);
            let source_path = source_template.path_buf(db);
            let uri = lsp_types::Uri::from_file_path(source_path.as_std_path())?;

            let ref_file = djls_source::File::new(db, source_path.clone(), 0);
            let line_index = ref_file.line_index(db);

            let tag = reference.tag(db);
            let tag_span = tag.span(db);
            let start_offset = tag_span.start_offset();
            let end_offset = tag_span.end_offset();

            let start_lc = line_index.to_line_col(start_offset);
            let end_lc = line_index.to_line_col(end_offset);

            let start_pos = lsp_types::Position {
                line: start_lc.line(),
                character: start_lc.column(),
            };
            let end_pos = lsp_types::Position {
                line: end_lc.line(),
                character: end_lc.column(),
            };

            Some(lsp_types::Location {
                uri,
                range: lsp_types::Range {
                    start: start_pos,
                    end: end_pos,
                },
            })
        })
        .collect();

    if locations.is_empty() {
        None
    } else {
        Some(locations)
    }
}

fn find_template_name_at_offset(nodes: &[Node], offset: Offset) -> Option<String> {
    for node in nodes {
        if let Node::Tag {
            name, bits, span, ..
        } = node
        {
            if (name == "extends" || name == "include") && span.contains(offset) {
                let template_str = bits.first()?;
                let template_name = template_str
                    .trim()
                    .trim_start_matches('"')
                    .trim_end_matches('"')
                    .trim_start_matches('\'')
                    .trim_end_matches('\'')
                    .to_string();
                return Some(template_name);
            }
        }
    }
    None
}

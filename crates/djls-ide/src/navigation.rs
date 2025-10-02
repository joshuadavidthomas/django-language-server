use djls_semantic::resolve_template;
use djls_semantic::ResolveResult;
use djls_source::File;
use djls_source::Offset;
use djls_templates::parse_template;
use djls_templates::Node;
use tower_lsp_server::lsp_types;

use crate::ext::SpanExt;
use crate::ext::Utf8PathExt;

pub fn goto_template_definition(
    db: &dyn djls_semantic::Db,
    file: File,
    offset: Offset,
) -> Option<lsp_types::GotoDefinitionResponse> {
    let nodelist = parse_template(db, file)?;

    let template_name = find_template_name_at_offset(nodelist.nodelist(db), offset)?;
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

pub fn find_template_references(
    db: &dyn djls_semantic::Db,
    file: File,
    offset: Offset,
) -> Option<Vec<lsp_types::Location>> {
    let nodelist = parse_template(db, file)?;

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

            let ref_file = djls_source::File::new(db, source_path.clone(), 0);
            let line_index = ref_file.line_index(db);

            let tag = reference.tag(db);
            let tag_span = tag.span(db);

            Some(lsp_types::Location {
                uri: source_path.to_lsp_uri()?,
                range: tag_span.to_lsp_range(line_index),
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

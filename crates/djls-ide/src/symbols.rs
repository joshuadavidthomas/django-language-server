use djls_semantic::OutlineItem;
use djls_semantic::OutlineKind;
use djls_semantic::TemplateOutline;
use djls_source::File;
use djls_source::LineIndex;
use tower_lsp_server::ls_types;

use crate::ext::SpanExt;

#[must_use]
pub fn document_symbols(db: &dyn djls_semantic::Db, file: File) -> Vec<ls_types::DocumentSymbol> {
    let Some(nodelist) = djls_templates::parse_template(db, file) else {
        return Vec::new();
    };

    let source = file.source(db);
    let tree = djls_semantic::build_template_tree(db, nodelist);
    let outline = djls_semantic::build_template_outline(db, nodelist, tree, source.as_str());
    outline_to_document_symbols(&outline, file.line_index(db))
}

pub(crate) fn outline_to_document_symbols(
    outline: &TemplateOutline,
    line_index: &LineIndex,
) -> Vec<ls_types::DocumentSymbol> {
    outline
        .items
        .iter()
        .map(|item| item_to_document_symbol(item, line_index))
        .collect()
}

fn item_to_document_symbol(item: &OutlineItem, line_index: &LineIndex) -> ls_types::DocumentSymbol {
    let children = (!item.children.is_empty()).then(|| {
        item.children
            .iter()
            .map(|child| item_to_document_symbol(child, line_index))
            .collect()
    });

    #[allow(deprecated)]
    ls_types::DocumentSymbol {
        name: item.label.clone(),
        detail: item.detail.clone(),
        kind: symbol_kind(item.kind),
        tags: None,
        deprecated: None,
        range: item.span.to_lsp_range(line_index),
        selection_range: item.selection_span.to_lsp_range(line_index),
        children,
    }
}

fn symbol_kind(kind: OutlineKind) -> ls_types::SymbolKind {
    match kind {
        OutlineKind::NamedRegion => ls_types::SymbolKind::NAMESPACE,
        OutlineKind::ControlFlow => ls_types::SymbolKind::OPERATOR,
        OutlineKind::TemplateReference | OutlineKind::FileReference => ls_types::SymbolKind::FILE,
        OutlineKind::LibraryImport => ls_types::SymbolKind::MODULE,
        OutlineKind::Callable | OutlineKind::RouteReference | OutlineKind::Filter => {
            ls_types::SymbolKind::FUNCTION
        }
        OutlineKind::Variable => ls_types::SymbolKind::VARIABLE,
    }
}

#[cfg(test)]
mod tests {
    use djls_source::Span;

    use super::*;

    #[test]
    fn outline_conversion_maps_kinds_ranges_and_children() {
        let source = "{% block content %}\n  {% include \"card.html\" %}\n{% endblock %}\n";
        let line_index = LineIndex::from(source);
        let outline = TemplateOutline {
            items: vec![OutlineItem {
                label: "content".to_string(),
                detail: Some("block".to_string()),
                kind: OutlineKind::NamedRegion,
                span: Span::saturating_from_bounds_usize(0, source.len() - 1),
                selection_span: Span::saturating_from_bounds_usize(3, 18),
                children: vec![
                    OutlineItem {
                        label: "card.html".to_string(),
                        detail: Some("include".to_string()),
                        kind: OutlineKind::TemplateReference,
                        span: Span::saturating_from_bounds_usize(22, 47),
                        selection_span: Span::saturating_from_bounds_usize(25, 45),
                        children: Vec::new(),
                    },
                    OutlineItem {
                        label: "user.username".to_string(),
                        detail: Some("variable".to_string()),
                        kind: OutlineKind::Variable,
                        span: Span::saturating_from_bounds_usize(48, 71),
                        selection_span: Span::saturating_from_bounds_usize(48, 61),
                        children: vec![OutlineItem {
                            label: "lower".to_string(),
                            detail: Some("filter".to_string()),
                            kind: OutlineKind::Filter,
                            span: Span::saturating_from_bounds_usize(62, 67),
                            selection_span: Span::saturating_from_bounds_usize(62, 67),
                            children: Vec::new(),
                        }],
                    },
                ],
            }],
        };

        let symbols = outline_to_document_symbols(&outline, &line_index);

        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "content");
        assert_eq!(symbols[0].detail.as_deref(), Some("block"));
        assert_eq!(symbols[0].kind, ls_types::SymbolKind::NAMESPACE);
        assert_eq!(symbols[0].range.start.line, 0);
        assert_eq!(symbols[0].range.end.line, 2);
        assert_eq!(symbols[0].selection_range.start.character, 3);

        let children = symbols[0].children.as_ref().expect("children should exist");
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].name, "card.html");
        assert_eq!(children[0].detail.as_deref(), Some("include"));
        assert_eq!(children[0].kind, ls_types::SymbolKind::FILE);
        assert_eq!(children[0].children, None);
        assert_eq!(children[1].name, "user.username");
        assert_eq!(children[1].kind, ls_types::SymbolKind::VARIABLE);
        assert_eq!(children[1].selection_range.start.line, 2);
        assert_eq!(children[1].selection_range.start.character, 0);
        assert_eq!(children[1].selection_range.end.line, 2);
        assert_eq!(children[1].selection_range.end.character, 13);
        let filters = children[1].children.as_ref().expect("filters should exist");
        assert_eq!(filters[0].name, "lower");
        assert_eq!(filters[0].kind, ls_types::SymbolKind::FUNCTION);
    }
}

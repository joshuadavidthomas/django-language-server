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

    let tree = djls_semantic::build_template_tree(db, nodelist);
    let outline = djls_semantic::build_template_outline(db, tree);
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

fn symbol_kind(_kind: OutlineKind) -> ls_types::SymbolKind {
    // LSP has no symbol kind for template tags. Use a single neutral kind so
    // clients do not render misleading categories like Package/Object/Function.
    ls_types::SymbolKind::KEY
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
                label: "block content".to_string(),
                detail: Some("block".to_string()),
                kind: OutlineKind::Block,
                span: Span::saturating_from_bounds_usize(0, source.len() - 1),
                selection_span: Span::saturating_from_bounds_usize(3, 18),
                children: vec![OutlineItem {
                    label: "include \"card.html\"".to_string(),
                    detail: Some("include".to_string()),
                    kind: OutlineKind::Include,
                    span: Span::saturating_from_bounds_usize(22, 47),
                    selection_span: Span::saturating_from_bounds_usize(25, 45),
                    children: Vec::new(),
                }],
            }],
        };

        let symbols = outline_to_document_symbols(&outline, &line_index);

        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "block content");
        assert_eq!(symbols[0].detail.as_deref(), Some("block"));
        assert_eq!(symbols[0].kind, ls_types::SymbolKind::KEY);
        assert_eq!(symbols[0].range.start.line, 0);
        assert_eq!(symbols[0].range.end.line, 2);
        assert_eq!(symbols[0].selection_range.start.character, 3);

        let children = symbols[0].children.as_ref().expect("children should exist");
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].name, "include \"card.html\"");
        assert_eq!(children[0].detail.as_deref(), Some("include"));
        assert_eq!(children[0].kind, ls_types::SymbolKind::KEY);
        assert_eq!(children[0].children, None);
    }
}

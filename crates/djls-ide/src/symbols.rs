use djls_semantic::OutlineItem;
use djls_source::File;
use djls_source::LineIndex;
use tower_lsp_server::ls_types;

use crate::ext::OutlineKindExt;
use crate::ext::SpanExt;

#[must_use]
pub fn document_symbols(db: &dyn djls_semantic::Db, file: File) -> Vec<ls_types::DocumentSymbol> {
    let Some(nodelist) = djls_templates::parse_template(db, file) else {
        return Vec::new();
    };

    let tree = djls_semantic::build_template_tree(db, nodelist);
    let outline = djls_semantic::build_template_outline(db, tree);
    let line_index = file.line_index(db);
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

    ls_types::DocumentSymbol {
        name: item.label.clone(),
        detail: item.detail.clone(),
        kind: item.kind.to_lsp_symbol_kind(),
        tags: None,
        // `deprecated` is itself deprecated by LSP 3.15 in favor of `tags`, but
        // `ls_types::DocumentSymbol` still includes the field for wire compatibility.
        // We set both to `None` because template outline items are not deprecated.
        #[allow(deprecated)]
        deprecated: None,
        range: item.span.to_lsp_range(line_index),
        selection_range: item.selection_span.to_lsp_range(line_index),
        children,
    }
}

#[cfg(test)]
mod tests {
    use djls_source::Span;

    use super::*;

    #[test]
    fn outline_conversion_maps_kinds_ranges_and_children() {
        let source = "{% block content %}\n  {% include \"card.html\" %}\n  {{ user.username|lower }}\n{% endblock %}\n";
        let line_index = LineIndex::from(source);
        let outline = djls_semantic::TemplateOutline {
            items: vec![OutlineItem {
                label: "content".to_string(),
                detail: Some("block".to_string()),
                kind: djls_semantic::OutlineKind::TemplateBlock,
                span: Span::saturating_from_bounds_usize(0, source.len() - 1),
                selection_span: Span::saturating_from_bounds_usize(3, 7),
                children: vec![
                    OutlineItem {
                        label: "card.html".to_string(),
                        detail: Some("include".to_string()),
                        kind: djls_semantic::OutlineKind::TemplateReference,
                        span: Span::saturating_from_bounds_usize(22, 47),
                        selection_span: Span::saturating_from_bounds_usize(25, 45),
                        children: Vec::new(),
                    },
                    OutlineItem {
                        label: "user.username".to_string(),
                        detail: None,
                        kind: djls_semantic::OutlineKind::Variable,
                        span: Span::saturating_from_bounds_usize(50, 74),
                        selection_span: Span::saturating_from_bounds_usize(53, 65),
                        children: vec![OutlineItem {
                            label: "lower".to_string(),
                            detail: None,
                            kind: djls_semantic::OutlineKind::Filter,
                            span: Span::saturating_from_bounds_usize(67, 71),
                            selection_span: Span::saturating_from_bounds_usize(67, 71),
                            children: Vec::new(),
                        }],
                    },
                ],
            }],
        };

        let symbols = outline
            .items
            .iter()
            .map(|item| item_to_document_symbol(item, &line_index))
            .collect::<Vec<_>>();

        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "content");
        assert_eq!(symbols[0].detail.as_deref(), Some("block"));
        assert_eq!(symbols[0].kind, ls_types::SymbolKind::NAMESPACE);
        assert_eq!(symbols[0].range.start.line, 0);
        assert_eq!(symbols[0].range.end.line, 3);
        assert_eq!(symbols[0].selection_range.start.character, 3);

        let children = symbols[0].children.as_ref().expect("children should exist");
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].name, "card.html");
        assert_eq!(children[0].detail.as_deref(), Some("include"));
        assert_eq!(children[0].kind, ls_types::SymbolKind::FILE);
        assert_eq!(children[0].children, None);
        assert_eq!(children[1].name, "user.username");
        assert_eq!(children[1].kind, ls_types::SymbolKind::VARIABLE);
        assert_eq!(children[1].detail, None);
        assert_eq!(children[1].selection_range.start.line, 2);
        assert_eq!(children[1].selection_range.start.character, 5);
        assert_eq!(children[1].selection_range.end.line, 2);
        assert_eq!(children[1].selection_range.end.character, 17);
        let filters = children[1].children.as_ref().expect("filters should exist");
        assert_eq!(filters[0].name, "lower");
        assert_eq!(filters[0].kind, ls_types::SymbolKind::FUNCTION);
    }
}

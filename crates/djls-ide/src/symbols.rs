use djls_source::File;
use tower_lsp_server::ls_types;

use crate::ext::TemplateOutlineExt;

#[must_use]
pub fn document_symbols(db: &dyn djls_semantic::Db, file: File) -> Vec<ls_types::DocumentSymbol> {
    let Some(nodelist) = djls_templates::parse_template(db, file) else {
        return Vec::new();
    };

    let tree = djls_semantic::build_template_tree(db, nodelist);
    let outline = djls_semantic::build_template_outline(db, tree);
    outline.to_lsp_document_symbols(file.line_index(db))
}

#[cfg(test)]
mod tests {
    use djls_semantic::OutlineItem;
    use djls_source::LineIndex;
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
                kind: djls_semantic::OutlineKind::NamedRegion,
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

        let symbols = outline.to_lsp_document_symbols(&line_index);

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

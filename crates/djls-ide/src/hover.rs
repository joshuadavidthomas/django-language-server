use djls_semantic::resolve_template;
use djls_semantic::InstalledSymbolCandidate;
use djls_semantic::InstalledSymbolOrigin;
use djls_semantic::ResolveResult;
use djls_semantic::TemplateSymbolKind;
use djls_semantic::TemplateSymbolName;
use djls_source::File;
use djls_source::Offset;
use djls_source::Span;
use djls_templates::parse_template;
use djls_templates::Node;
use tower_lsp_server::ls_types;

use crate::ext::SpanExt;

pub fn hover(db: &dyn djls_semantic::Db, file: File, offset: Offset) -> Option<ls_types::Hover> {
    let source = file.source(db);
    let line_index = file.line_index(db);
    let nodelist = parse_template(db, file)?;

    let node = nodelist
        .nodelist(db)
        .iter()
        .find(|node| node.full_span().contains(offset))?;

    match node {
        Node::Tag { name, bits, span } => {
            let name_span = span.with_length_usize_saturating(name.len());
            if name_span.contains(offset) {
                return symbol_hover(db, name, TemplateSymbolKind::Tag, name_span, line_index);
            }

            if matches!(name.as_str(), "extends" | "include") {
                let bit = bits.first()?;
                let bit_span = find_bit_span(source.as_str(), *span, bit)?;
                if bit_span.contains(offset) {
                    let template_name = unquote(bit);
                    return template_reference_hover(db, &template_name, bit_span, line_index);
                }
            }

            None
        }
        Node::Variable { filters, .. } => filters.iter().find_map(|filter| {
            let name_span = filter.span.with_length_usize_saturating(filter.name.len());
            if name_span.contains(offset) {
                symbol_hover(
                    db,
                    &filter.name,
                    TemplateSymbolKind::Filter,
                    name_span,
                    line_index,
                )
            } else {
                None
            }
        }),
        Node::Comment { .. } | Node::Text { .. } | Node::Error { .. } => None,
    }
}

fn symbol_hover(
    db: &dyn djls_semantic::Db,
    name: &str,
    kind: TemplateSymbolKind,
    span: Span,
    line_index: &djls_source::LineIndex,
) -> Option<ls_types::Hover> {
    let libraries = db.template_libraries();
    let candidates: Vec<_> = libraries
        .installed_symbol_candidates(kind)
        .into_iter()
        .filter(|candidate| candidate.symbol.name() == name)
        .collect();

    let markdown = if candidates.is_empty() {
        let discovered = TemplateSymbolName::parse(name)
            .ok()
            .and_then(|name| {
                libraries
                    .discovered_symbol_candidates_by_name(kind)
                    .and_then(|mut candidates| candidates.remove(&name))
            })
            .map(|candidates| {
                candidates
                    .into_iter()
                    .map(|candidate| {
                        format!(
                            "- `{}` from `{}`",
                            candidate.library_name.as_str(),
                            candidate.app_module.as_str(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if discovered.is_empty() {
            return None;
        }

        render_discovered_symbol_hover(name, kind, &discovered)
    } else {
        render_installed_symbol_hover(name, kind, &candidates)
    };

    Some(ls_types::Hover {
        contents: ls_types::HoverContents::Markup(ls_types::MarkupContent {
            kind: ls_types::MarkupKind::Markdown,
            value: markdown,
        }),
        range: Some(span.to_lsp_range(line_index)),
    })
}

fn template_reference_hover(
    db: &dyn djls_semantic::Db,
    template_name: &str,
    span: Span,
    line_index: &djls_source::LineIndex,
) -> Option<ls_types::Hover> {
    let markdown = match resolve_template(db, template_name) {
        ResolveResult::Found(template) => {
            let path = template.path_buf(db);
            format!("### Template `{template_name}`\n\nResolved to `{path}`")
        }
        ResolveResult::NotFound { tried, .. } => {
            if tried.is_empty() {
                return None;
            }

            let tried = tried
                .iter()
                .map(|path| format!("- `{path}`"))
                .collect::<Vec<_>>()
                .join("\n");
            format!("### Template `{template_name}`\n\nNot found. Tried:\n\n{tried}")
        }
    };

    Some(ls_types::Hover {
        contents: ls_types::HoverContents::Markup(ls_types::MarkupContent {
            kind: ls_types::MarkupKind::Markdown,
            value: markdown,
        }),
        range: Some(span.to_lsp_range(line_index)),
    })
}

fn render_installed_symbol_hover(
    name: &str,
    kind: TemplateSymbolKind,
    candidates: &[InstalledSymbolCandidate],
) -> String {
    let title = match kind {
        TemplateSymbolKind::Tag => format!("### Tag `{name}`"),
        TemplateSymbolKind::Filter => format!("### Filter `{name}`"),
    };

    let doc = candidates
        .iter()
        .find_map(|candidate| candidate.symbol.doc())
        .map(str::trim)
        .filter(|doc| !doc.is_empty());

    let origins = candidates
        .iter()
        .map(render_origin)
        .collect::<Vec<_>>()
        .join("\n");

    match doc {
        Some(doc) => format!("{title}\n\n{doc}\n\n{origins}"),
        None => format!("{title}\n\n{origins}"),
    }
}

fn render_discovered_symbol_hover(
    name: &str,
    kind: TemplateSymbolKind,
    discovered: &[String],
) -> String {
    let title = match kind {
        TemplateSymbolKind::Tag => format!("### Tag `{name}`"),
        TemplateSymbolKind::Filter => format!("### Filter `{name}`"),
    };

    format!(
        "{title}\n\nDiscovered in installed template libraries:\n\n{}",
        discovered.join("\n"),
    )
}

fn render_origin(candidate: &InstalledSymbolCandidate) -> String {
    match &candidate.origin {
        InstalledSymbolOrigin::Builtin { module } => format!("Built-in: `{}`", module.as_str()),
        InstalledSymbolOrigin::Loadable { load_name } => {
            format!("Load with: `{{% load {} %}}`", load_name.as_str())
        }
    }
}

fn find_bit_span(source: &str, content_span: Span, bit: &str) -> Option<Span> {
    let content_start = content_span.start_usize();
    let content_end = content_span.end() as usize;
    let content = source.get(content_start..content_end)?;
    let relative_start = content.find(bit)?;
    Some(Span::saturating_from_parts_usize(
        content_start + relative_start,
        bit.len(),
    ))
}

fn unquote(raw: &str) -> String {
    let trimmed = raw.trim();
    trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
        })
        .unwrap_or(trimmed)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(
        kind: TemplateSymbolKind,
        name: &str,
        doc: Option<&str>,
        origin: InstalledSymbolOrigin,
    ) -> InstalledSymbolCandidate {
        InstalledSymbolCandidate {
            symbol: djls_semantic::TemplateSymbol {
                kind,
                name: TemplateSymbolName::parse(name).unwrap(),
                definition: djls_semantic::SymbolDefinition::Unknown,
                doc: doc.map(str::to_string),
            },
            origin,
        }
    }

    #[test]
    fn tag_hover_uses_docs_and_builtin_origin() {
        let candidates = vec![candidate(
            TemplateSymbolKind::Tag,
            "if",
            Some("Evaluate a condition."),
            InstalledSymbolOrigin::Builtin {
                module: djls_semantic::PyModuleName::parse("django.template.defaulttags").unwrap(),
            },
        )];

        let markdown = render_installed_symbol_hover("if", TemplateSymbolKind::Tag, &candidates);

        assert!(markdown.contains("### Tag `if`"));
        assert!(markdown.contains("Evaluate a condition."));
        assert!(markdown.contains("Built-in: `django.template.defaulttags`"));
    }

    #[test]
    fn filter_hover_uses_load_origin() {
        let candidates = vec![candidate(
            TemplateSymbolKind::Filter,
            "intcomma",
            None,
            InstalledSymbolOrigin::Loadable {
                load_name: djls_semantic::LibraryName::parse("humanize").unwrap(),
            },
        )];

        let markdown =
            render_installed_symbol_hover("intcomma", TemplateSymbolKind::Filter, &candidates);

        assert!(markdown.contains("### Filter `intcomma`"));
        assert!(markdown.contains("Load with: `{% load humanize %}`"));
    }

    #[test]
    fn unquote_strips_template_reference_quotes() {
        assert_eq!(unquote("\"base.html\""), "base.html");
        assert_eq!(unquote("'base.html'"), "base.html");
        assert_eq!(unquote(" base.html "), "base.html");
    }
}

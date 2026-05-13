use djls_semantic::resolve_template;
use djls_semantic::InstalledSymbolCandidate;
use djls_semantic::InstalledSymbolOrigin;
use djls_semantic::LoadKind;
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
            if matches!(name.as_str(), "extends" | "include") {
                let bit = bits.first()?;
                let bit_span = find_bit_span(source.as_str(), *span, bit)?;
                if bit_span.contains(offset) {
                    let template_name = unquote(bit);
                    return template_reference_hover(db, &template_name, bit_span, line_index);
                }
            }

            if name == "load" {
                if let Some((library, library_span)) =
                    load_library_at_offset(source.as_str(), *span, bits, offset)
                {
                    return library_hover(db, &library, library_span, line_index);
                }
            }

            symbol_hover(
                db,
                name,
                TemplateSymbolKind::Tag,
                node.full_span(),
                line_index,
            )
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
                            "Load with `{{% load {} %}}`.",
                            candidate.library_name.as_str()
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        render_discovered_symbol_hover(&discovered)?
    } else {
        render_installed_symbol_hover(&candidates)?
    };

    Some(markdown_hover(markdown, span, line_index))
}

fn markdown_hover(
    markdown: String,
    span: Span,
    line_index: &djls_source::LineIndex,
) -> ls_types::Hover {
    ls_types::Hover {
        contents: ls_types::HoverContents::Markup(ls_types::MarkupContent {
            kind: ls_types::MarkupKind::Markdown,
            value: markdown,
        }),
        range: Some(span.to_lsp_range(line_index)),
    }
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
            format!("Resolved to `{path}`")
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
            format!("Template not found.\n\nTried:\n\n{tried}")
        }
    };

    Some(markdown_hover(markdown, span, line_index))
}

fn library_hover(
    db: &dyn djls_semantic::Db,
    library_name: &str,
    span: Span,
    line_index: &djls_source::LineIndex,
) -> Option<ls_types::Hover> {
    let library = db
        .template_libraries()
        .best_loadable_library_str(library_name)?;
    Some(markdown_hover(
        library.module().as_str().to_string(),
        span,
        line_index,
    ))
}

fn render_installed_symbol_hover(candidates: &[InstalledSymbolCandidate]) -> Option<String> {
    let doc = candidates
        .iter()
        .find_map(|candidate| candidate.symbol.doc())
        .map(str::trim)
        .filter(|doc| !doc.is_empty());

    if let Some(doc) = doc {
        return Some(doc.to_string());
    }

    let load_hints = candidates.iter().filter_map(load_hint).collect::<Vec<_>>();

    if load_hints.is_empty() {
        None
    } else {
        Some(load_hints.join("\n"))
    }
}

fn render_discovered_symbol_hover(discovered: &[String]) -> Option<String> {
    if discovered.is_empty() {
        None
    } else {
        Some(discovered.join("\n"))
    }
}

fn load_hint(candidate: &InstalledSymbolCandidate) -> Option<String> {
    match &candidate.origin {
        InstalledSymbolOrigin::Builtin { .. } => None,
        InstalledSymbolOrigin::Loadable { load_name } => {
            Some(format!("Load with `{{% load {} %}}`.", load_name.as_str()))
        }
    }
}

fn load_library_at_offset(
    source: &str,
    content_span: Span,
    bits: &[String],
    offset: Offset,
) -> Option<(String, Span)> {
    match djls_semantic::parse_load_bits(bits)? {
        LoadKind::FullLoad { libraries } => {
            library_bit_at_offset(source, content_span, bits, offset, &libraries)
        }
        LoadKind::SelectiveImport { library, .. } => {
            library_bit_at_offset(source, content_span, bits, offset, &[library])
        }
    }
}

fn library_bit_at_offset(
    source: &str,
    content_span: Span,
    bits: &[String],
    offset: Offset,
    libraries: &[String],
) -> Option<(String, Span)> {
    bit_spans(source, content_span, bits)
        .into_iter()
        .find(|(bit, span)| libraries.contains(bit) && span.contains(offset))
}

fn find_bit_span(source: &str, content_span: Span, bit: &str) -> Option<Span> {
    bit_spans(source, content_span, &[bit.to_string()])
        .into_iter()
        .next()
        .map(|(_, span)| span)
}

fn bit_spans(source: &str, content_span: Span, bits: &[String]) -> Vec<(String, Span)> {
    let content_start = content_span.start_usize();
    let content_end = content_span.end() as usize;
    let Some(content) = source.get(content_start..content_end) else {
        return Vec::new();
    };

    let mut spans = Vec::new();
    let mut search_start = 0;

    for bit in bits {
        let Some(relative_start) = content[search_start..].find(bit) else {
            continue;
        };
        let relative_start = search_start + relative_start;
        spans.push((
            bit.clone(),
            Span::saturating_from_parts_usize(content_start + relative_start, bit.len()),
        ));
        search_start = relative_start + bit.len();
    }

    spans
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
    fn tag_hover_prefers_docstring() {
        let candidates = vec![candidate(
            TemplateSymbolKind::Tag,
            "if",
            Some("Evaluate a condition."),
            InstalledSymbolOrigin::Builtin {
                module: djls_semantic::PyModuleName::parse("django.template.defaulttags").unwrap(),
            },
        )];

        let markdown = render_installed_symbol_hover(&candidates);

        assert_eq!(markdown.as_deref(), Some("Evaluate a condition."));
    }

    #[test]
    fn filter_hover_falls_back_to_load_hint() {
        let candidates = vec![candidate(
            TemplateSymbolKind::Filter,
            "intcomma",
            None,
            InstalledSymbolOrigin::Loadable {
                load_name: djls_semantic::LibraryName::parse("humanize").unwrap(),
            },
        )];

        let markdown = render_installed_symbol_hover(&candidates);

        assert_eq!(
            markdown.as_deref(),
            Some("Load with `{% load humanize %}`.")
        );
    }

    #[test]
    fn load_library_at_offset_handles_full_load() {
        let source = "{% load static i18n %}";
        let bits = vec!["static".to_string(), "i18n".to_string()];
        let result = load_library_at_offset(source, Span::new(3, 16), &bits, Offset::new(9));

        assert!(matches!(result, Some((library, _)) if library == "static"));
    }

    #[test]
    fn load_library_at_offset_handles_selective_load() {
        let source = "{% load trans from i18n %}";
        let bits = vec!["trans".to_string(), "from".to_string(), "i18n".to_string()];
        let symbol = load_library_at_offset(source, Span::new(3, 20), &bits, Offset::new(9));
        let library = load_library_at_offset(source, Span::new(3, 20), &bits, Offset::new(21));

        assert!(symbol.is_none());
        assert!(matches!(library, Some((library, _)) if library == "i18n"));
    }

    #[test]
    fn unquote_strips_template_reference_quotes() {
        assert_eq!(unquote("\"base.html\""), "base.html");
        assert_eq!(unquote("'base.html'"), "base.html");
        assert_eq!(unquote(" base.html "), "base.html");
    }
}

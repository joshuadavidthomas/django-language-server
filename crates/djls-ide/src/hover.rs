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

    let (markdown, span) = HoverTarget::from_node(node, source.as_str(), offset)?.render(db)?;
    Some(markdown_hover(markdown, span, line_index))
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HoverTarget<'a> {
    TemplateReference {
        raw_name: &'a str,
        span: Span,
    },
    LoadLibrary {
        name: String,
        span: Span,
    },
    Symbol {
        name: &'a str,
        kind: TemplateSymbolKind,
        span: Span,
    },
}

impl<'a> HoverTarget<'a> {
    fn from_node(node: &'a Node, source: &str, offset: Offset) -> Option<Self> {
        match node {
            Node::Tag { name, bits, span } => Some(Self::from_tag(
                name,
                bits,
                *span,
                node.full_span(),
                source,
                offset,
            )),
            Node::Variable { filters, .. } => filters.iter().find_map(|filter| {
                let span = filter.span.with_length_usize_saturating(filter.name.len());
                span.contains(offset).then_some(Self::Symbol {
                    name: &filter.name,
                    kind: TemplateSymbolKind::Filter,
                    span,
                })
            }),
            Node::Comment { .. } | Node::Text { .. } | Node::Error { .. } => None,
        }
    }

    fn from_tag(
        name: &'a str,
        bits: &'a [String],
        content_span: Span,
        full_span: Span,
        source: &str,
        offset: Offset,
    ) -> Self {
        if matches!(name, "extends" | "include") {
            if let Some(bit) = bits.first() {
                if let Some(span) = find_bit_span(source, content_span, bit) {
                    if span.contains(offset) {
                        return Self::TemplateReference {
                            raw_name: bit,
                            span,
                        };
                    }
                }
            }
        }

        if name == "load" {
            if let Some((name, span)) = load_library_at_offset(source, content_span, bits, offset) {
                return Self::LoadLibrary { name, span };
            }
        }

        Self::Symbol {
            name,
            kind: TemplateSymbolKind::Tag,
            span: full_span,
        }
    }

    fn render(self, db: &dyn djls_semantic::Db) -> Option<(String, Span)> {
        match self {
            Self::TemplateReference { raw_name, span } => {
                Some((template_reference_markdown(db, &unquote(raw_name))?, span))
            }
            Self::LoadLibrary { name, span } => Some((library_markdown(db, &name)?, span)),
            Self::Symbol { name, kind, span } => Some((symbol_markdown(db, name, kind)?, span)),
        }
    }
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

fn symbol_markdown(
    db: &dyn djls_semantic::Db,
    name: &str,
    kind: TemplateSymbolKind,
) -> Option<String> {
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

        if discovered.is_empty() {
            return None;
        }
        discovered.join("\n")
    } else {
        render_installed_symbol_hover(&candidates)?
    };

    Some(markdown)
}

fn template_reference_markdown(db: &dyn djls_semantic::Db, template_name: &str) -> Option<String> {
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

    Some(markdown)
}

fn library_markdown(db: &dyn djls_semantic::Db, library_name: &str) -> Option<String> {
    let library = db
        .template_libraries()
        .best_loadable_library_str(library_name)?;
    Some(library.module().as_str().to_string())
}

fn render_installed_symbol_hover(candidates: &[InstalledSymbolCandidate]) -> Option<String> {
    let candidate = candidates
        .iter()
        .find(|candidate| {
            candidate
                .symbol
                .doc()
                .is_some_and(|doc| !doc.trim().is_empty())
        })
        .or_else(|| candidates.first())?;

    let name = candidate.symbol.name();
    let signature = match candidate.symbol.kind {
        TemplateSymbolKind::Tag => format!("{{% {name} %}}"),
        TemplateSymbolKind::Filter => format!("{{{{ value|{name} }}}}"),
    };
    let mut sections = vec![format!("```htmldjango\n{signature}\n```")];

    if let Some(doc) = candidate
        .symbol
        .doc()
        .map(format_docstring)
        .filter(|doc| !doc.is_empty())
    {
        sections.push(doc);
    }

    sections.extend(
        candidates
            .iter()
            .filter_map(|candidate| match &candidate.origin {
                InstalledSymbolOrigin::Builtin { .. } => None,
                InstalledSymbolOrigin::Loadable { load_name } => {
                    Some(format!("Load with `{{% load {} %}}`.", load_name.as_str()))
                }
            }),
    );

    if let InstalledSymbolOrigin::Loadable { .. } = candidate.origin {
        match &candidate.symbol.definition {
            djls_semantic::SymbolDefinition::Module(module) => {
                sections.push(format!("`{}`", module.as_str()));
            }
            djls_semantic::SymbolDefinition::Exact { file }
            | djls_semantic::SymbolDefinition::LibraryFile(file) => {
                sections.push(format!("`{file}`"));
            }
            djls_semantic::SymbolDefinition::Unknown => {}
        }
    }

    Some(sections.join("\n\n"))
}

fn format_docstring(doc: &str) -> String {
    let doc = doc.trim().replace("``", "`");
    let mut lines = Vec::new();
    let mut in_code_block = false;
    let mut pending_code_block = false;

    for line in doc.lines() {
        let trimmed_end = line.trim_end();
        let trimmed = trimmed_end.trim_start();

        if let Some(prefix) = trimmed_end.strip_suffix("::") {
            let prefix = prefix.trim_end();
            if prefix.is_empty() {
                pending_code_block = true;
            } else {
                lines.push(format!("{prefix}:"));
                pending_code_block = true;
            }
            continue;
        }

        let is_indented = trimmed_end.starts_with("    ") || trimmed_end.starts_with('\t');
        if pending_code_block && trimmed_end.is_empty() {
            lines.push(String::new());
            continue;
        }
        if pending_code_block && is_indented {
            lines.push("```htmldjango".to_string());
            in_code_block = true;
            pending_code_block = false;
        }

        if in_code_block {
            if is_indented || trimmed_end.is_empty() {
                lines.push(trimmed.to_string());
                continue;
            }
            close_code_block(&mut lines);
            in_code_block = false;
        }

        pending_code_block = false;
        lines.push(trimmed_end.to_string());
    }

    if in_code_block {
        close_code_block(&mut lines);
    }

    lines.join("\n").trim().to_string()
}

fn close_code_block(lines: &mut Vec<String>) {
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.push("```".to_string());
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
        .find(|(bit, span)| libraries.iter().any(|library| library == bit) && span.contains(offset))
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
        let definition = match &origin {
            InstalledSymbolOrigin::Builtin { .. } => djls_semantic::SymbolDefinition::Unknown,
            InstalledSymbolOrigin::Loadable { load_name } => {
                djls_semantic::SymbolDefinition::Module(
                    djls_semantic::PyModuleName::parse(&format!(
                        "django.contrib.{0}.templatetags.{0}",
                        load_name.as_str()
                    ))
                    .unwrap(),
                )
            }
        };

        InstalledSymbolCandidate {
            symbol: djls_semantic::TemplateSymbol {
                kind,
                name: TemplateSymbolName::parse(name).unwrap(),
                definition,
                doc: doc.map(str::to_string),
            },
            origin,
        }
    }

    #[test]
    fn tag_hover_includes_signature_and_docstring() {
        let candidates = vec![candidate(
            TemplateSymbolKind::Tag,
            "if",
            Some("Evaluate a condition."),
            InstalledSymbolOrigin::Builtin {
                module: djls_semantic::PyModuleName::parse("django.template.defaulttags").unwrap(),
            },
        )];

        let markdown = render_installed_symbol_hover(&candidates);

        assert_eq!(
            markdown.as_deref(),
            Some("```htmldjango\n{% if %}\n```\n\nEvaluate a condition."),
        );
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
            Some("```htmldjango\n{{ value|intcomma }}\n```\n\nLoad with `{% load humanize %}`.\n\n`django.contrib.humanize.templatetags.humanize`"),
        );
    }

    #[test]
    fn format_docstring_converts_restructured_text_examples() {
        let doc = r#"Load a template and render it with the current context.

Example::

    {% include "foo/some_include" %}
    {% include "foo/some_include" with bar="BAZZ!" %}

Use the ``only`` argument::

    {% include "foo/some_include" only %}"#;

        let formatted = format_docstring(doc);

        assert!(formatted.contains("Example:"));
        assert!(formatted.contains("```htmldjango\n{% include \"foo/some_include\" %}"));
        assert!(formatted.contains("Use the `only` argument:"));
        assert!(formatted.ends_with("```"));
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

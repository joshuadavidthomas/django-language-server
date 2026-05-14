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
    Some(ls_types::Hover {
        contents: ls_types::HoverContents::Markup(ls_types::MarkupContent {
            kind: ls_types::MarkupKind::Markdown,
            value: markdown,
        }),
        range: Some(span.to_lsp_range(line_index)),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HoverTarget<'a> {
    TemplateReference {
        name: &'a str,
        span: Span,
    },
    LoadLibrary {
        name: &'a str,
        span: Span,
    },
    Symbol {
        name: &'a str,
        kind: TemplateSymbolKind,
        span: Span,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TagHoverKind {
    TemplateReference,
    Load,
    Symbol,
}

impl TagHoverKind {
    fn from_name(name: &str) -> Self {
        match name {
            "extends" | "include" => Self::TemplateReference,
            "load" => Self::Load,
            _ => Self::Symbol,
        }
    }
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
        match TagHoverKind::from_name(name) {
            TagHoverKind::TemplateReference => {
                if let Some(bit) = bits.first() {
                    let content_start = content_span.start_usize();
                    let content_end = content_span.end() as usize;
                    if let Some(relative_start) = source
                        .get(content_start..content_end)
                        .and_then(|content| content.find(bit))
                    {
                        let span = Span::saturating_from_parts_usize(
                            content_start + relative_start,
                            bit.len(),
                        );
                        if span.contains(offset) {
                            let name = bit
                                .trim()
                                .strip_prefix('"')
                                .and_then(|s| s.strip_suffix('"'))
                                .or_else(|| {
                                    bit.trim()
                                        .strip_prefix('\'')
                                        .and_then(|s| s.strip_suffix('\''))
                                })
                                .unwrap_or_else(|| bit.trim());
                            return Self::TemplateReference { name, span };
                        }
                    }
                }
            }
            TagHoverKind::Load => {
                let libraries = match djls_semantic::parse_load_bits(bits) {
                    Some(LoadKind::FullLoad { libraries }) => libraries,
                    Some(LoadKind::SelectiveImport { library, .. }) => vec![library],
                    None => Vec::new(),
                };

                let content_start = content_span.start_usize();
                let content_end = content_span.end() as usize;
                if let Some(content) = source.get(content_start..content_end) {
                    let mut search_start = 0;
                    for bit in bits {
                        let Some(relative_start) = content[search_start..].find(bit) else {
                            continue;
                        };
                        let relative_start = search_start + relative_start;
                        let span = Span::saturating_from_parts_usize(
                            content_start + relative_start,
                            bit.len(),
                        );
                        if libraries.iter().any(|library| library == bit) && span.contains(offset) {
                            return Self::LoadLibrary {
                                name: bit.as_str(),
                                span,
                            };
                        }
                        search_start = relative_start + bit.len();
                    }
                }
            }
            TagHoverKind::Symbol => {}
        }

        Self::Symbol {
            name,
            kind: TemplateSymbolKind::Tag,
            span: full_span,
        }
    }

    fn render(self, db: &dyn djls_semantic::Db) -> Option<(String, Span)> {
        match self {
            Self::TemplateReference { name, span } => {
                let markdown = match resolve_template(db, name) {
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
                Some((markdown, span))
            }
            Self::LoadLibrary { name, span } => {
                let library = db.template_libraries().best_loadable_library_str(name)?;
                Some((library.module().as_str().to_string(), span))
            }
            Self::Symbol { name, kind, span } => {
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

                Some((markdown, span))
            }
        }
    }
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
            while lines.last().is_some_and(String::is_empty) {
                lines.pop();
            }
            lines.push("```".to_string());
            in_code_block = false;
        }

        pending_code_block = false;
        lines.push(trimmed_end.to_string());
    }

    if in_code_block {
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.push("```".to_string());
    }

    lines.join("\n").trim().to_string()
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
}

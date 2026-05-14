use djls_semantic::resolve_template;
use djls_semantic::InstalledSymbolCandidate;
use djls_semantic::InstalledSymbolOrigin;
use djls_semantic::ResolveResult;
use djls_semantic::TemplateLibraries;
use djls_semantic::TemplateSymbolKind;
use djls_semantic::TemplateSymbolName;
use djls_source::File;
use djls_source::Offset;
use tower_lsp_server::ls_types;

use crate::context::OffsetContext;
use crate::ext::SpanExt;

pub fn hover(db: &dyn djls_semantic::Db, file: File, offset: Offset) -> Option<ls_types::Hover> {
    let (markdown, span) = match OffsetContext::from_offset(db, file, offset) {
        OffsetContext::TemplateReference { name, span } => {
            let markdown = match resolve_template(db, &name) {
                ResolveResult::Found(template) => {
                    let path = template.path_buf(db);
                    format!("Resolved to `{path}`")
                }
                ResolveResult::NotFound { tried, .. } => {
                    // No tried paths means the project did not provide template loader
                    // locations, so there is nothing useful to show.
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
        OffsetContext::LoadLibrary { name, span } => {
            let library = db.template_libraries().best_loadable_library_str(&name)?;
            Some((library.module().as_str().to_string(), span))
        }
        OffsetContext::LoadSymbol { name, span } => Some((
            render_symbol_hover(db.template_libraries(), &name, None)?,
            span,
        )),
        OffsetContext::Tag { name, span } => Some((
            render_symbol_hover(
                db.template_libraries(),
                &name,
                Some(TemplateSymbolKind::Tag),
            )?,
            span,
        )),
        OffsetContext::Filter { name, span } => Some((
            render_symbol_hover(
                db.template_libraries(),
                &name,
                Some(TemplateSymbolKind::Filter),
            )?,
            span,
        )),
        OffsetContext::BlockDefinition { .. }
        | OffsetContext::BlockReference { .. }
        | OffsetContext::Variable { .. }
        | OffsetContext::Comment { .. }
        | OffsetContext::Text { .. }
        | OffsetContext::None => None,
    }?;

    Some(ls_types::Hover {
        contents: ls_types::HoverContents::Markup(ls_types::MarkupContent {
            kind: ls_types::MarkupKind::Markdown,
            value: markdown,
        }),
        range: Some(span.to_lsp_range(file.line_index(db))),
    })
}

fn render_symbol_hover(
    libraries: &TemplateLibraries,
    name: &str,
    kind: Option<TemplateSymbolKind>,
) -> Option<String> {
    let kinds: &[TemplateSymbolKind] = match kind {
        Some(TemplateSymbolKind::Tag) => &[TemplateSymbolKind::Tag],
        Some(TemplateSymbolKind::Filter) => &[TemplateSymbolKind::Filter],
        None => &[TemplateSymbolKind::Tag, TemplateSymbolKind::Filter],
    };

    let candidates: Vec<_> = kinds
        .iter()
        .flat_map(|kind| libraries.installed_symbol_candidates(*kind))
        .filter(|candidate| candidate.symbol.name() == name)
        .collect();

    if !candidates.is_empty() {
        return render_installed_symbol_hover(&candidates);
    }

    let name = TemplateSymbolName::parse(name).ok()?;
    let discovered = kinds
        .iter()
        .filter_map(|kind| {
            libraries
                .discovered_symbol_candidates_by_name(*kind)
                .and_then(|mut candidates| candidates.remove(&name))
        })
        .flatten()
        .map(|candidate| {
            format!(
                "Load with `{{% load {} %}}`.",
                candidate.library_name.as_str()
            )
        })
        .collect::<Vec<_>>();

    if discovered.is_empty() {
        None
    } else {
        Some(discovered.join("\n"))
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
    // Django's built-in tag and filter docstrings use reStructuredText examples
    // for template syntax, so hover fences those blocks as htmldjango.
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
    use std::collections::BTreeMap;

    use djls_semantic::Knowledge;
    use djls_semantic::LibraryName;
    use djls_semantic::LibraryOrigin;
    use djls_semantic::PyModuleName;
    use djls_semantic::TemplateLibraries;
    use djls_semantic::TemplateLibrary;

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
    fn discovered_symbol_hover_shows_load_hint() {
        let mut library = TemplateLibrary::new_discovered(
            LibraryName::parse("humanize").unwrap(),
            LibraryOrigin {
                app: PyModuleName::parse("django.contrib.humanize").unwrap(),
                module: PyModuleName::parse("django.contrib.humanize.templatetags.humanize")
                    .unwrap(),
                path: "django/contrib/humanize/templatetags/humanize.py".into(),
            },
        );
        library.symbols.push(djls_semantic::TemplateSymbol {
            kind: TemplateSymbolKind::Filter,
            name: TemplateSymbolName::parse("intcomma").unwrap(),
            definition: djls_semantic::SymbolDefinition::Unknown,
            doc: None,
        });
        let libraries = TemplateLibraries {
            active_knowledge: Knowledge::Unknown,
            discovery_knowledge: Knowledge::Known,
            loadable: BTreeMap::from([(LibraryName::parse("humanize").unwrap(), vec![library])]),
            builtins: BTreeMap::new(),
        };

        let markdown =
            render_symbol_hover(&libraries, "intcomma", Some(TemplateSymbolKind::Filter));

        assert_eq!(
            markdown.as_deref(),
            Some("Load with `{% load humanize %}`.")
        );
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
    fn format_docstring_closes_example_at_end_of_docstring() {
        let doc = "Example::\n\n    {% load static %}";

        let formatted = format_docstring(doc);

        assert_eq!(
            formatted,
            "Example:\n\n```htmldjango\n{% load static %}\n```"
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

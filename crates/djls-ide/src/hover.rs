use djls_semantic::FindTemplateResult;
use djls_semantic::InstalledSymbolCandidate;
use djls_semantic::InstalledSymbolOrigin;
use djls_semantic::SemanticOffsetContext;
use djls_semantic::TemplateLibraries;
use djls_semantic::TemplateSymbolKind;
use djls_semantic::find_template;
use djls_source::File;
use djls_source::Offset;
use tower_lsp_server::ls_types;

use crate::ext::SpanExt;

pub fn hover(db: &dyn djls_semantic::Db, file: File, offset: Offset) -> Option<ls_types::Hover> {
    let (markdown, span) = match SemanticOffsetContext::from_offset(db, file, offset) {
        SemanticOffsetContext::TemplateReference {
            name: template_name,
            span,
        } => {
            let project = db.project()?;
            let name = template_name.name(db);

            let mut sections = vec![format!("```text\n(template) \"{name}\"\n```")];

            match find_template(db, project, template_name) {
                FindTemplateResult::Found(origin) => {
                    let path = origin.path_buf(db);
                    sections.push(format!("Resolved to `{path}`"));
                }
                FindTemplateResult::DoesNotExist(error) => {
                    // No tried paths means the project did not provide template loader
                    // locations, so there is nothing useful to show.
                    if error.tried.is_empty() {
                        return None;
                    }

                    let tried = error
                        .tried
                        .iter()
                        .map(|source| format!("- `{}`", source.path))
                        .collect::<Vec<_>>()
                        .join("\n");
                    sections.push(format!("Template not found.\n\nTried:\n\n{tried}"));
                }
            }
            Some((sections.join("\n---\n"), span))
        }
        SemanticOffsetContext::LoadLibrary { name, span } => {
            let library = db.template_libraries().best_loadable_library_str(&name)?;
            Some((
                format!(
                    "```text\n(library) {name}\n```\n---\n```python\n{}\n```",
                    library.module().as_str()
                ),
                span,
            ))
        }
        SemanticOffsetContext::LoadSymbol { name, span } => Some((
            render_symbol_hover(db.template_libraries(), &name, None)?,
            span,
        )),
        SemanticOffsetContext::Tag { name, span } => Some((
            render_symbol_hover(
                db.template_libraries(),
                &name,
                Some(TemplateSymbolKind::Tag),
            )?,
            span,
        )),
        SemanticOffsetContext::Filter { name, span } => Some((
            render_symbol_hover(
                db.template_libraries(),
                &name,
                Some(TemplateSymbolKind::Filter),
            )?,
            span,
        )),
        SemanticOffsetContext::Variable { .. } | SemanticOffsetContext::None => None,
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

    None
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
    let kind = match candidate.symbol.kind {
        TemplateSymbolKind::Tag => "tag",
        TemplateSymbolKind::Filter => "filter",
    };
    let mut sections = vec![format!("```text\n({kind}) {name}\n```")];

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
                    Some(format!("Requires `{{% load {} %}}`.", load_name.as_str()))
                }
            }),
    );

    Some(sections.join("\n---\n"))
}

fn format_docstring(doc: &str) -> String {
    // Django's built-in tag and filter docstrings use reStructuredText examples
    // for template syntax, so hover fences those blocks as htmldjango.
    let doc = doc.trim_matches(['\n', '\r']);
    let common_indent = doc
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    let doc = doc
        .lines()
        .map(|line| line.get(common_indent..).unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("``", "`");
    let mut lines = Vec::new();
    let mut in_code_block = false;
    let mut pending_code_block = false;

    for line in doc.lines() {
        let trimmed_end = line.trim_end();
        let trimmed = trimmed_end.trim_start();
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
            lines.push(String::new());
            in_code_block = false;
        }

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
    use djls_semantic::TemplateSymbolName;

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
            Some("```text\n(tag) if\n```\n---\nEvaluate a condition."),
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
            Some("```text\n(filter) intcomma\n```\n---\nRequires `{% load humanize %}`."),
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
    fn format_docstring_dedents_docstring_before_fencing_examples() {
        let doc = r#"    It is possible to store the translated string into a variable::

        {% translate "this is a test" as var %}
        {{ var }}

    Contextual translations are also supported::

        {% translate "this is a test" context "greeting" %}"#;

        let formatted = format_docstring(doc);

        assert!(formatted.contains("{{ var }}\n```"));
        assert!(formatted.contains("Contextual translations are also supported:"));
        assert!(
            formatted
                .contains("```htmldjango\n{% translate \"this is a test\" context \"greeting\" %}")
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

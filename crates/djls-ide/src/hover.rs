use std::fmt::Write as _;

use djls_project::FindTemplateResult;
use djls_project::LoadableLibraryLookup;
use djls_project::TemplateLibraries;
use djls_project::TemplateSymbolAvailability;
use djls_project::TemplateSymbolCandidate;
use djls_project::TemplateSymbolKind;
use djls_project::template_resolution;
use djls_semantic::SemanticOffsetContext;
use djls_semantic::resolve_reference_name;
use djls_source::File;
use djls_source::Offset;
use tower_lsp_server::ls_types;

use crate::ext::SpanExt;

pub fn hover(db: &dyn djls_semantic::Db, file: File, offset: Offset) -> Option<ls_types::Hover> {
    let (markdown, span) = match SemanticOffsetContext::from_offset(db, file, offset) {
        SemanticOffsetContext::TemplateReference {
            name: template_name,
            kind,
            span,
        } => {
            let project = db.project()?;
            let name = template_name.name(db);
            let resolution = template_resolution(db, project);
            let resolved_template_name =
                resolve_reference_name(db, resolution, file, template_name, kind)?;

            let mut sections = vec![format!("```text\n(template) \"{name}\"\n```")];

            match resolution.resolve(db, resolved_template_name) {
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
                        .map(|path| format!("- `{path}`"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    sections.push(format!("Template not found.\n\nTried:\n\n{tried}"));
                }
                FindTemplateResult::Inconclusive(search) => {
                    // An incomplete search must not claim the template is missing; report what
                    // the search did establish instead.
                    let mut message = "Template search is incomplete.".to_string();
                    if !search.possible_origins.is_empty() {
                        let possible = search
                            .possible_origins
                            .iter()
                            .map(|origin| format!("- `{}`", origin.path_buf(db)))
                            .collect::<Vec<_>>()
                            .join("\n");
                        let _ = write!(message, "\n\nPossible matches:\n\n{possible}");
                    }
                    sections.push(message);
                }
            }
            Some((sections.join("\n---\n"), span))
        }
        SemanticOffsetContext::LoadLibrary { name, span } => {
            let LoadableLibraryLookup::Found(library) =
                db.template_libraries().loadable_library_str(&name)
            else {
                return None;
            };
            Some((
                format!(
                    "```text\n(library) {name}\n```\n---\n```python\n{}\n```",
                    library.module_name_str()
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
        .flat_map(|kind| libraries.template_symbol_candidates(*kind))
        .filter(|candidate| candidate.symbol.name() == name)
        .collect();

    if !candidates.is_empty() {
        return render_template_symbol_hover(&candidates);
    }

    None
}

fn render_template_symbol_hover(candidates: &[TemplateSymbolCandidate]) -> Option<String> {
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
            .filter_map(|candidate| match &candidate.availability {
                TemplateSymbolAvailability::Builtin { module: _ } => None,
                TemplateSymbolAvailability::RequiresLoad { load_name } => {
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
    use std::collections::HashMap;

    use super::*;

    #[derive(Clone, Copy)]
    enum TestOrigin {
        Builtin(&'static str),
        Installed(&'static str),
    }

    fn candidate(
        kind: TemplateSymbolKind,
        name: &str,
        doc: Option<&str>,
        origin: TestOrigin,
    ) -> TemplateSymbolCandidate {
        let mut libraries = HashMap::new();
        let mut builtins = Vec::new();
        let mut fixture = match (kind, origin) {
            (TemplateSymbolKind::Tag, TestOrigin::Builtin(module)) => {
                builtins.push(module.to_string());
                djls_testing::builtin_tag(name, module)
            }
            (TemplateSymbolKind::Filter, TestOrigin::Builtin(module)) => {
                builtins.push(module.to_string());
                djls_testing::builtin_filter(name, module)
            }
            (TemplateSymbolKind::Tag, TestOrigin::Installed(load_name)) => {
                let module = format!("django.contrib.{load_name}.templatetags.{load_name}");
                libraries.insert(load_name.to_string(), module.clone());
                djls_testing::library_tag(name, load_name, &module)
            }
            (TemplateSymbolKind::Filter, TestOrigin::Installed(load_name)) => {
                let module = format!("django.contrib.{load_name}.templatetags.{load_name}");
                libraries.insert(load_name.to_string(), module.clone());
                djls_testing::library_filter(name, load_name, &module)
            }
        };
        if let Some(doc) = doc {
            fixture["doc"] = doc.into();
        }

        let (tags, filters) = match kind {
            TemplateSymbolKind::Tag => (vec![fixture], Vec::new()),
            TemplateSymbolKind::Filter => (Vec::new(), vec![fixture]),
        };
        let db = djls_testing::TestDatabase::new();
        let libraries =
            djls_testing::make_template_libraries(&db, &tags, &filters, &libraries, &builtins);

        libraries
            .template_symbol_candidates(kind)
            .into_iter()
            .find(|candidate| candidate.symbol.name() == name)
            .expect("candidate should exist")
    }

    #[test]
    fn tag_hover_includes_signature_and_docstring() {
        let candidates = vec![candidate(
            TemplateSymbolKind::Tag,
            "if",
            Some("Evaluate a condition."),
            TestOrigin::Builtin("django.template.defaulttags"),
        )];

        let markdown = render_template_symbol_hover(&candidates);

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
            TestOrigin::Installed("humanize"),
        )];

        let markdown = render_template_symbol_hover(&candidates);

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

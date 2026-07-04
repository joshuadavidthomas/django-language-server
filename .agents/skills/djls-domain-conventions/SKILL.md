---
name: djls-domain-conventions
description: Use when editing Django template parser tags, validation errors, diagnostics, environment scanning, inspector data, Python environment inventory, or DJLS domain model code. Handles project-specific semantic and parser conventions.
---

# DJLS Domain Conventions

Use this for changes to template parsing, semantic validation, environment scanning, diagnostics, and inspector-derived project data.

## Template parser

- `Node::Tag.bits` excludes the tag name.
- Example: `{% load i18n %}` becomes `name: "load"`, `bits: ["i18n"]`.
- Functions processing `bits` work with tag arguments only.
- Extracted text and its span are computed by the same parse, in the crate that owns the syntax. Do not re-derive quote/content spans in semantic or IDE consumers.

## Environment scanning

- External rule/model scan orchestration lives in `crates/djls-db/src/scanning.rs`.
- Project context, inspector data, and module resolution live in `crates/djls-semantic/src/project/`:
  - `project.rs` defines `Project`.
  - `symbols.rs` defines `TemplateLibraries`, `TemplateLibrary`, `TemplateSymbol`, and inspector response types.
  - `resolve.rs` handles Python module and model-file discovery.
  - `python.rs` handles interpreter discovery.
- Static Python extraction lives in `crates/djls-semantic/src/python/`.

## Validation errors

Quick-fix edits are derived from typed `ValidationError` values against the current Salsa snapshot inside the request handler. Keep `Diagnostic.data` as `None`; do not serialize internal error or edit shapes into the LSP wire contract.

When adding or removing `ValidationError` variants, update:

- `errors.rs`
- `djls-ide/src/diagnostics.rs` S-code mapping
- test helpers

Use:

```bash
rg "ValidationError" crates/ -g '*.rs'
```

## Semantic model

- Cross-file semantic queries depend on small per-file derived products (e.g. `template_symbols`), never on other files' full parses directly.
- Code over config: tag-spec knowledge acquisition prefers static extraction from Python source; tagspec config is a last-resort escape hatch, never primary.
- Semantic features bind to `TagRole`, never tag names.
- Derived project facts carry explicit completeness/termination states (e.g. `ChainEnd`, `TemplateDirStatus`); consumers branch on them rather than inferring from emptiness.

## Local runtime state

- Server logs: `~/.cache/djls/djls.log.YYYY-MM-DD`
- Inspector cache: `~/.cache/djls/inspector/`

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

## Environment scanning

- Environment scan functions live in `djls-project/src/scanning.rs`.
- Environment types live in `djls-python/src/environment/types.rs`:
  - `EnvironmentInventory`
  - `EnvironmentLibrary`
  - `EnvironmentSymbol`

## Validation errors

When adding or removing `ValidationError` variants, update:

- `errors.rs`
- `djls-ide/src/diagnostics.rs` S-code mapping
- test helpers

Use:

```bash
rg "ValidationError" crates/ -g '*.rs'
```

## Local runtime state

- Server logs: `~/.cache/djls/djls.log.YYYY-MM-DD`
- Inspector cache: `~/.cache/djls/inspector/`

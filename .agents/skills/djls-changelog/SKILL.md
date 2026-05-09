---
name: djls-changelog
description: Use when adding release notes, changelog entries, user-facing changes, internal changelog notes, or preparing a PR/release for django-language-server. Handles Keep a Changelog formatting and entry style.
---

# DJLS Changelog

Use this for any user-facing change or release-note work.

## Rules

- Every user-facing change needs a changelog entry in the same commit or PR.
- Entries go under `[Unreleased]` in the appropriate section:
  - `Added`
  - `Changed`
  - `Deprecated`
  - `Removed`
  - `Fixed`
  - `Security`
- Keep entries short and factual: what changed, not why.
- Use past tense verbs: `Added`, `Fixed`, `Removed`, `Bumped`.
- Prefix internal-only changes with `**Internal**:` and list them after user-facing entries.
- Backtick-wrap code identifiers: crate names, types, commands, config keys.

## Example

```markdown
### Fixed

- Fixed `{% load %}` completions for custom template tag libraries.
- **Internal**: Refactored `djls-semantic` validation helpers.
```

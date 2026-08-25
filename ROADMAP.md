# Django Language Server Roadmap

## Scope

Django Language Server is a template-first language server. This roadmap records what works now, what should ship next, and which broader Django features depend on stronger Project Facts.

## Status legend

- ✅ Supported
- 🚧 Partially supported
- 📅 Planned
- 💭 Considering
- 🚫 Not planned

## Strategy

Finish the remaining high-value Django template features, then separate generic Python analysis from Django Project Facts before expanding into URLs, settings, Models, Template Context, and ORM queries. Prefer conservative diagnostics when Project Facts are incomplete or dynamic. Build rename, code lens, and type-driven features only after symbol identity and references can support safe edits.

## Current support

| Area | Status | Current behavior |
|---|---|---|
| Template diagnostics | ✅ | Syntax, tag and Filter Definition availability, Filter Arity, block structure, and template-usage diagnostics |
| Template completion | 🚧 | Template Tags, tag arguments, Template Libraries, selective loads, Filters, and literal Template Names; parent Inheritance Block names remain |
| Hover | ✅ | Template Tags, Filters, Template Libraries, selective loads, and Template References |
| Go to definition | ✅ | Literal Template References, Inheritance Blocks, Template Libraries, Template Tags, and Filters when the target is definite |
| Find references | 🚧 | Template References and definite Inheritance Block chains; other Django domains remain |
| Document links | ✅ | Resolved Template References and Template Library names |
| Document symbols and folding | ✅ | Template structure outlines and folding ranges |
| Formatting | ✅ | Opt-in whole-document formatting through `djangofmt` |
| Code actions | 🚧 | Missing and ambiguous `{% load %}` fixes and mismatched `{% endblock %}` name renames; safe `{% extends %}` placement remains |
| Project discovery | ✅ | Static Django Discovery without importing Django or running project Python |
| Configuration | ✅ | Project, diagnostics, formatter, environment, and manual tag-spec settings |
| Workspace folders | ✅ | Multi-root workspace discovery and workspace-folder changes |
| File watching | 🚧 | Open buffers and filesystem changes invalidate source facts; project-status feedback remains limited |

## Now

### Block-name completion

**Status:** 📅 Planned

Complete `{% block name %}` arguments from definite parent Templates. The inheritance query layer already provides the nearest inherited block definitions; the remaining work is IDE completion and LSP coverage.

### Safe `{% extends %}` placement action

**Status:** 📅 Planned

Offer the S122 quick fix only when moving `{% extends %}` ahead of preceding content preserves unrelated source text. This is the final slice in the current code-action plan bank.

### Python analysis ownership

**Status:** 📅 Planned

Finish the bounded settings and environment inputs already in flight, then separate generic Python analysis from Django project analysis. Broader URL, view, settings, and Model work resumes after that boundary is in place.

## Next

### Template Partial intelligence

**Status:** 🚧 Partially supported

The semantic layer recognizes Template Partial definitions, but editor navigation, completion, and references do not expose them yet. Plan those consumers before implementation.

### URL and static asset intelligence

**Status:** 💭 Considering

Start with URL-name and static-file completion, then add conservative unresolved-reference diagnostics, hover, and navigation from extracted Project Facts.

### Settings support

**Status:** 💭 Considering

Start with known-setting hover and clear typo diagnostics. Python edit-context completion can wait until the Python analysis boundary has settled.

### Django Model facts in editor features

**Status:** 💭 Considering

Expose qualified Django Model identity, field and relation spans, and relation-target navigation and hover. Finish the active proxy/MTI representation and conformance work before adding editor consumers.

## Later

### Template Context and Template Variable intelligence

**Status:** 💭 Considering

Collect local bindings from `{% for %}`, `{% with %}`, and `as` aliases before attempting view-derived Template Context or member types.

### ORM query intelligence

**Status:** 💭 Considering

Complete and validate field paths, relations, and lookup suffixes for common query shapes only after Model facts and Python expression ownership are stable.

### Other LSP features

- Complete Filter arguments when their callable metadata is precise enough.
- Start rename with definite Inheritance Block chains.
- Start document highlights with block names in one Template.
- Start workspace symbols with Templates and Inheritance Blocks.
- Consider code lens, signature help, semantic tokens, selection ranges, and inlay hints after their source facts are stable.

## Deferred or not planned

- **Broad Python IDE replacement:** 🚫 DJLS complements Python language servers.
- **Type definition and type hierarchy:** defer until Template Variable type inference exists.
- **Call hierarchy:** leave Python call relationships to Python language servers.
- **Range and on-type formatting:** defer until whole-document formatting has wider use.
- **Notebook documents, monikers, document colors, and inline debugger values:** no Django-specific use.
- **Full Jinja analysis:** demand-gated; supporting Django's Jinja backend requires a separate syntax layer.

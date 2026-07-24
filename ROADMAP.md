# Django Language Server Roadmap

## Scope

This roadmap describes the feature direction for Django Language Server: what is supported today, what should come next, and which larger capabilities are intentionally deferred.

The project is currently a template-first Django language server. The near-term goal is to make Django template editing feel complete before expanding into broader Django/Python intelligence.

## Status Legend

- ✅ Supported
- 🚧 Partially supported
- 📅 Planned
- 💭 Considering
- 🚫 Not planned

## Strategy

Finish high-value template IDE features first: template-name completion, block navigation, block references, and quick fixes. Broader Django intelligence should build from static project facts through settings, URLs/static assets, model facts, view context, and finally template variable types.

Prefer conservative diagnostics over false positives when project facts are partial or dynamic. Defer rename, code lens, and type-driven features until references and symbol identity are strong enough to make them safe.

## Django Domain Roadmap

### Now

#### Template reference polish

**Status:** 🚧 Partially supported

`{% extends %}` and `{% include %}` references should feel complete across completion, navigation, and editor links. Document links are supported for resolved `{% extends %}`, `{% include %}`, and `{% load %}` template-library names. Go to definition resolves literal `{% extends %}` and `{% include %}` names and sends exact origin ranges to clients that support definition links. Remaining first-pass polish should add template-name completion from discovered templates.

#### Inheritance block intelligence

**Status:** 📅 Planned

Users should be able to navigate and understand Django template inheritance without manual search. The first pass should support going to parent block definitions, finding child block overrides, and completing block names from parent templates.

#### Code actions for safe diagnostics

**Status:** 📅 Planned

Common template diagnostics should offer quick fixes when the edit is deterministic. Start with missing `{% load %}` statements, unmatched block names, and invalid `{% extends %}` placement.

### Next

#### URL and static asset intelligence

**Status:** 💭 Considering

`{% url %}` and `{% static %}` should eventually gain completion, diagnostics, hover, and navigation. Start with URL-name discovery/completion, unresolved URL diagnostics, and static file completion from known static roots.

#### Settings support

**Status:** 💭 Considering

Django settings modules should get useful hover, typo diagnostics, and eventually completions. Start with known-setting hover and obvious misspelling diagnostics before taking on Python edit-context completion.

#### Django model facts in editor features

**Status:** 💭 Considering

Django model and relation facts should become visible through hover, navigation, diagnostics, and completions. The first useful slice is qualified model identity, field and relation spans, and relation-target navigation.

### Later

#### Template context and variable intelligence

**Status:** 💭 Considering

Template variables should eventually support member completion, hover, and goto based on view context, context processors, model facts, and local template bindings. Early slices can collect local bindings from `{% for %}`, `{% with %}`, and `as` aliases before attempting broader type inference.

#### ORM query intelligence

**Status:** 💭 Considering

Common ORM expressions should eventually complete and validate field paths, relations, and lookup suffixes. Start with simple `Model.objects.filter(...)`, `order_by(...)`, `select_related(...)`, and `prefetch_related(...)` shapes.

## LSP Capability Roadmap

### Language Features

#### Diagnostics

**Status:** ✅ Supported

Template syntax and semantic validation are supported, including tag/filter availability, filter arity, block structure, and template usage rules. The next step is quick fixes, richer related information, and conservative diagnostics for new Django domains.

#### Completion

**Status:** 🚧 Partially supported

The server completes template tag names, tag arguments, `{% load %}` library names, selective load symbols, and filter names. Next up are template-name completions, block-name completions, filter arguments, URL names, static files, and eventually template variables.

#### Hover

**Status:** ✅ Supported

Hover is supported for template tags, filters, libraries, selectively loaded symbols, and template references. Later hovers should cover variables, models, settings, URL names, and static assets once those facts exist.

#### Go to definition / declaration / implementation

**Status:** 🚧 Partially supported

Go to definition works for literal template references in `{% extends %}` and `{% include %}`, Template Library arguments in `{% load %}`, selective-load symbols, and available Template Tags and Filters with definite local Python declarations. Clients that support definition links receive exact origin and declaration ranges; older clients receive plain locations. Ambiguous definitions and dynamic, imported, or member callables do not produce guessed targets. The next improvement is parent block navigation across inheritance.

#### Find references

**Status:** 🚧 Partially supported

Find references works for templates used by `{% extends %}` and `{% include %}`. Block override/reference search should come next, with variable, URL, static asset, settings, and model references later.

#### Document symbols

**Status:** ✅ Supported

Document symbols are supported for template structure outlines. They can become richer once block and inheritance relationships are modeled across files.

#### Document links

**Status:** ✅ Supported

Document links make resolved `{% extends %}` and `{% include %}` template names clickable in editors, and link `{% load %}` template-library names to their resolved `templatetags/*.py` source files. Static asset and URL links can follow once those domains have facts.

#### Folding range

**Status:** ✅ Supported

Folding is supported for Django template blocks, comments, and import regions. Keep this aligned with future template-structure changes.

#### Formatting

**Status:** ✅ Supported

Opt-in whole-document Django template formatting is supported. Range formatting and on-type formatting should wait until whole-document formatting has enough real-world validation.

#### Code actions

**Status:** 📅 Planned

Code actions should start as deterministic quick fixes for existing diagnostics. Missing loads, unmatched block names, and invalid extends placement are the best first targets.

#### Rename

**Status:** 💭 Considering

Rename is most plausible first for inheritance blocks across an extends/override chain. It should wait until block definitions and references are reliable.

#### Selection range

**Status:** 💭 Considering

Selection range could provide semantic expand-selection through template tokens, tag arguments, full tags, block contents, full blocks, parent blocks, and the whole document. This is useful polish after the structural model is stable enough for editor interaction.

#### Semantic tokens

**Status:** 💭 Considering

Semantic tokens could provide Django-aware token categories for richer template highlighting. This is lower leverage than navigation, completion, and code actions.

#### Inlay hints

**Status:** 💭 Considering

Inlay hints could show resolved template paths, URL targets, setting values, or inferred model/context types inline. They should wait for stronger domain facts and careful noise control.

#### Signature help

**Status:** 💭 Considering

Signature help could show parameter hints while typing custom tag and filter arguments. It depends on richer tag/filter argument metadata.

#### Document highlight

**Status:** 💭 Considering

Document highlight could show matching template names, block names, local bindings, and later variables. Single-file block-name highlights are the smallest useful first slice.

#### Code lens

**Status:** 💭 Considering

Code lens could show inline counts and actions for template inheritance, block overrides, and template hierarchy. It should consume block references and inheritance relationships rather than create a separate model.

### Workspace Features

#### Workspace symbols

**Status:** 💭 Considering

Workspace symbols could search templates, inheritance blocks, URL names, settings, and model symbols across the project. Template and inheritance block symbols are the natural first slice.

#### Configuration

**Status:** ✅ Supported

Project settings, diagnostics configuration, formatter configuration, environment inputs, and manual tag-spec fallbacks are supported. Prefer better inference over new override knobs, and add configuration only for real user decisions or unavoidable project-specific facts.

#### Workspace folders

**Status:** ✅ Supported

Workspace folder support and workspace folder change handling are supported. Future work should keep multi-root behavior aligned with project discovery and source-file identity.

#### File watching and file operations

**Status:** 🚧 Partially supported

Open document buffers, filesystem-backed project discovery, and source-file invalidation are partially supported. The goal is to keep project facts, template indexes, and editor features responsive to file changes.

#### Execute command

**Status:** 💭 Considering

Execute commands may be useful for hierarchy views, project-status inspection, or future refactorings. Add commands only when a concrete feature needs them.

## Priority Order

1. Template-name completion for `{% extends %}` and `{% include %}`.
2. Block navigation across inheritance.
3. Block references and block-name completion.
4. Code actions for existing diagnostics.
5. URL/static completions and diagnostics.
6. Settings hover and typo diagnostics.
7. Model facts surfaced through navigation and hover.
8. Template context and variable intelligence.

## Deferred or Not Planned

- **Type definition / type hierarchy:** defer unless template type inference becomes real.
- **Call hierarchy:** better handled by Python language servers.
- **Monikers:** no clear Django use.
- **Document colors:** not Django-specific.
- **Inline values:** debugger-adjacent; not relevant right now.
- **Notebook documents:** not relevant.
- **On-type formatting:** defer until whole-document formatting is proven.
- **Broad Python IDE replacement:** not a goal. DJLS should complement Python language servers, not replace them.

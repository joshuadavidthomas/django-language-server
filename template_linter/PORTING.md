# Porting Notes (Python → Rust)

This repository contains a Python prototype (`template_linter/`) that proves:
- static extraction of Django template tag/filter validation rules from Python source (AST-only)
- static validation of template syntax (tags, filters, block structure)
- validation across a large real-world corpus (libraries + projects)

The long-term plan is to port the implementation to Rust (and **not** keep the
Python implementation as the primary engine). This document tracks practical
porting considerations so we don't drift into a Python-only shape that's painful
to reimplement.

Non-goals of this document:
- prescribing a full Rust crate architecture
- proposing Rust APIs in detail
- discussing UI/editor integration

## Core Invariants (Must Preserve)

- **No runtime template execution**
  - Never render templates.
  - Never import/execute tag/filter code to "validate" it.
  - Tokenization may use a lexer; validation must remain static.

- **Rules come from the actual Python code**
  - Prefer extracting constraints from Python source (AST patterns).
  - Avoid hand-authored specs (TOML/JSON) except where explicitly unavoidable.
  - **This replaces djls's current handcoded `builtins.rs` TagSpecs** with
    dynamically extracted rules from Django/third-party source.

- **Conservatism over false positives**
  - When resolution is ambiguous (e.g. collisions without a registry), prefer:
    - omit validation for the ambiguous symbol, or
    - downgrade severity / provide "cannot prove" diagnostics.

- **LSP value without runtime truth**
  - The goal is not "full validation," it's "max useful static feedback":
    - crisp syntax + scoping errors
    - best-effort inference for editor features (completions, hover, jump-to-def)
  - Any inferred "types" must be represented as evidence, not guarantees.

## Exception Categories in Django Templates

Understanding what exceptions Django raises and where helps scope what's extractable:

### Extractable (Compile-Time, Single-Tag)
These are raised in `do_xxx()` compile functions and can be extracted via AST:
- Token count checks: `if len(bits) < 4: raise TemplateSyntaxError(...)`
- Keyword position checks: `if bits[2] != "as": raise TemplateSyntaxError(...)`
- Option validation: while loops checking known options, duplicates, required args
- parse_bits validation: `library.py` validates simple_tag/inclusion_tag signatures

### Extractable (Compile-Time, Expression)
These are raised by `IfParser` (smartif.py) during expression parsing:
- Operator/operand syntax errors: `{% if and x %}`, `{% if x == %}`
- Dangling operators and unused trailing tokens (e.g. `{% if not %}`, `{% if x y %}`)
- **Phase 4.7 added this capability.**

### Deferred (Compile-Time, Template-Wide State)
These require tracking state across multiple tags within a template:
- Cycle name existence: `{% resetcycle foo %}` checks `parser._named_cycle_nodes`
- Partial definitions: `{% partial foo %}` checks against `{% partialdef %}` tags
- **Deferred to Rust implementation (Phase 6)** - requires semantic analysis layer.

### Out of Scope (Render-Time)
These depend on runtime context and cannot be statically validated:
- Variable resolution failures in `Node.render()`
- Type coercion errors (e.g., `widthratio` expects numbers)
- Format string errors in `blocktranslate`
- **These can be addressed with type hints** if view introspection provides types.

## What Exists Today (Python Prototype)

- Extraction:
  - Tag rules from Django/third-party `templatetags/**/*.py` by AST inspection:
    - TemplateSyntaxError conditions → `TagValidation` rules
    - supports preconditions, compound rules, token "views" (slices/pops), loops
    - special support for parse_bits-derived signatures (simple_tag/inclusion_tag/simple_block_tag)
  - Filter signatures (arg counts) from AST
  - Opaque block detection (content should not be parsed/validated)
  - Structural extraction for block tags:
    - delimiter tags and end tags derived from `parser.parse((...))`
    - includes dynamic patterns like `f"end{tag_name}"`

- Validation:
  - tokenizes templates (Django lexer when available; regex fallback exists)
  - validates tag syntax against extracted rules
  - validates filter usage/arg counts in `{{ }}` and `{% %}` contexts
  - validates block structure (nesting/ordering of delimiter tags)
  - supports `{% load %}` scoping (position-aware for tags; filters currently final-set)

- Planned (Phase 4.7):
  - Expression parsing for `{% if %}` / `{% elif %}` tags
  - Validates operator/operand syntax (not types or variable existence)

- Corpus harness:
  - `template_linter/corpus/manifest.toml` pins packages (sdists) and repos (git commits)
  - `corpus/sync.py` downloads/copies `templatetags/**/*.py` and `templates/**/*`
  - tests validate extraction and template validation across corpus

- Runtime registry emulation (optional):
  - `corpus/sync.py` writes `.runtime_registry.json` into corpus entries as a static approximation.
  - `corpus/inspect_runtime.py` can output `.runtime_registry.json` for a real project env:
    - `libraries`: `{load_name: module_path}`
    - `builtins`: ordered list of builtin library module paths
  - `src/template_linter/resolution/runtime_registry.py` consumes that JSON without executing tag/filter code

## LSP-Oriented Static Analysis (Planned / Porting-Relevant)

Beyond validation errors, a language server benefits from *inference* that is
explicitly best-effort:

- Symbol availability (position-aware):
  - which tags/filters are in scope at this point (builtins + `{% load %}`)
  - collisions resolved conservatively without a registry, accurately with one
- Template-local bindings:
  - `{% with %}` bindings
  - `{% for %}` introduces loop variables and `forloop`
  - `as var` patterns in tags (assignment semantics)
- Evidence-based variable "types":
  - track possible types as unions / unknowns
  - attach confidence so the UI can distinguish hints from errors
  - sources of evidence can include:
    - inclusion tags (returned context dict shape is high signal)
    - template-local flow (with/for/as)
    - (optional) host-supplied project inspection (views/context processors)

Porting implication:
- design the Rust data model so these analyses can share the same tokenizer and
  the same resolved registry/scoping machinery.

## Porting Strategy (Pragmatic Phasing)

### Phase A: Data Model Parity
Recreate the minimal set of types and serialized forms that let us test parity.

Porting-critical types/concepts:
- Token stream model for tags: `TemplateTag` (name, tokens, line, raw)
- Rule model: `TagValidation` and rule primitives (token-count checks, regex checks, keyword positioning, etc.)
- Filter model: `FilterSpec` (arg counts; possibly variadic/kw-only notes)
- Opaque blocks: `OpaqueBlockSpec` (end_tags + match suffix)
- Structural rules: `ConditionalInnerTagRule` + `BlockTagSpec` (start/middle/end tags)

Suggested approach:
- Ensure every Rust struct can be serialized to JSON for golden tests.
- Keep ordering deterministic (stable sort keys).

### Phase B: Tokenization + Template Traversal
Implement template tokenization compatible with the Python validator:
- Must produce equivalent `{% %}` / `{{ }}` tag tokens and line numbers.
- Must honor opaque blocks (do not parse inside).
- Must support `{% load %}` tags as first-class tokens (for scoping).

Notes:
- The Python prototype sometimes uses Django's Lexer when available. In Rust we
  should expect to implement our own tokenizer (or reuse djls's lexer/parser if
  it exists), but we must preserve the semantics around:
  - comment/verbatim skipping
  - block tags and their stop tokens
  - string literal handling inside tokens (as best-effort)

### Phase C: Static AST Extraction in Rust
Reimplement extraction using a Python AST parser in Rust (e.g. ruff's Python AST).

Key extraction capabilities to preserve:
- Registration discovery:
  - `@register.simple_tag(...)`, `register.simple_tag(func, name="alias")`
  - `register.tag("name", SomeClass.handle)` (callable expressions)
  - same for filters
- Rule discovery:
  - recognize `TemplateSyntaxError(...)` raise sites
  - associate raises with prior guards that imply constraints
  - carry "token view" operations forward (slices/pops) in a conservative way
- Structural discovery:
  - stop tokens from `parser.parse((...))` including:
    - tuple literals
    - names/constants
    - f-string style dynamic `end{tag_name}` patterns (best-effort)
- Opaque block discovery:
  - `parser.skip_past("end...")` and "manual scan loop" patterns
- Expression parsing (for `{% if %}` / `{% elif %}`):
  - port the Python prototype's expression validator
  - handle operators, literals, variables (operands treated opaquely)
  - produce clear error messages for syntax errors

### Phase D: Load Resolution + Runtime Registry
Implement `{% load %}` scoping and collision behavior:
- Static mode:
  - library index from `templatetags/**/*.py` basenames under an entry root
  - collision policy: union but omit collisions when ambiguous
- Runtime registry mode (optional):
  - build library index from `{name: module}` mapping
  - apply ordered builtins bundle (later wins)
  - strict unknowns become meaningful in real projects

### Phase E: Corpus + Parity Testing
Use the corpus to keep the port honest and prevent regressions.

Minimum parity goal:
- given the same corpus inputs, Rust and Python produce:
  - broadly matching error counts and "unknown" classifications
  - no explosions on extraction

## Known Porting Risks / Gotchas

- **Ordering semantics**
  - Django's "later override earlier" behavior must be preserved:
    - `{% load %}` later loads override earlier
    - builtins ordering matters ("later wins")

- **Ambiguous collisions**
  - Without INSTALLED_APPS order, same-named libraries are ambiguous.
  - The current behavior is conservative (omit collisions); preserve that default.

- **Tokenizer differences**
  - Even small tokenization differences can change validation outcomes.
  - Keep a dedicated "tokenization parity" test set (tiny templates with known token splits).

- **F-strings / dynamic end tags**
  - Third-party tags (notably Wagtail-like patterns) use `f"end{tag_name}"`.
  - We extract stop tokens best-effort; do not regress this.

- **Class-based tags and inheritance**
  - Real-world tags frequently register `SomeClass.handle`.
  - Best-effort method resolution within a module exists in Python; Rust port
    should plan an equivalent conservative resolution.

- **Filter extraction scope**
  - Filter validation in Python currently validates `{{ }}` and `{% %}` contexts.
  - `{% load %}` scoping for filters is currently "final set", not position-aware.
    - This is a known semantic mismatch; decide whether to keep it for parity or
      improve it in the port (but do so intentionally and with tests).

- **Inference can't be "right," it can only be "useful"**
  - For LSP-type features, prefer:
    - evidence graphs with unknowns and confidence
    - no "definitely" unless statically provable
  - This avoids turning best-effort hints into noisy errors.

- **"Hardcodes" policy**
  - Any unavoidable hardcoding must live in one place (see `HARDCODES.md` and
    `src/template_linter/overrides.py`).
  - In Rust, retain an equivalent single override module/table.

## djls Integration Gaps

### Inspector Data Gaps

The djls inspector (`crates/djls-project/inspector/queries.py`) needs updates to
provide the data template_linter requires for proper `{% load %}` scoping:

- **Builtins vs libraries distinction**
  - template_linter needs to know which tags are **builtins** (always available)
    vs **libraries** (require `{% load %}`).
  - djls currently flattens `engine.template_builtins` and `engine.libraries`
    into a single `templatetags` list, losing this distinction.
  - **Fix**: The `templatetags` query (or a new query) should return:
    - `builtins`: list of `{name, module}` for tags from `engine.template_builtins`
    - `libraries`: dict of `{load_name: [{name, module}, ...]}` for tags from `engine.libraries`
  - Alternatively, add an `is_builtin` field to each `TemplateTag`.
  - **Implemented (djls)**: add a separate `template_registry` query that returns:
    - `libraries`: `Engine.libraries` mapping `{load_name: module_path}`
    - `builtins`: ordered `Engine.builtins` list (later wins)
    This directly matches template_linter's `.runtime_registry.json` shape.

- **User-configured builtins**
  - Django allows adding builtins via `settings.TEMPLATES[...]['OPTIONS']['builtins']`.
  - The inspector must expose `Engine.builtins` (which includes these), not just
    hardcoded Django defaults.

- **Library load name**
  - djls provides `module` (e.g., `"django.templatetags.static"`) but not the
    `{% load %}` name (e.g., `"static"`).
  - The load name can be derived: `module.rsplit('.', 1)[-1]` for standard cases.
  - However, custom library names configured via `TEMPLATES['OPTIONS']['libraries']`
    would require exposing `engine.libraries` keys explicitly.
  - **Implemented (djls)**: `template_registry.libraries` uses the `engine.libraries`
    keys, so custom load names are preserved.

### TagSpecs Replacement Strategy

djls currently uses handcoded `TagSpec` definitions in `crates/djls-semantic/src/templatetags/builtins.rs`.
template_linter's extracted rules are designed to replace this approach:

| djls TagSpecs (current) | template_linter extraction (replacement) |
|-------------------------|------------------------------------------|
| Handcoded arg structure | Extracted from `do_xxx()` AST patterns |
| Manually maintained | Auto-generated from Django/library source |
| Limited to builtins | Works for any library with source access |
| Static at compile time | Can be regenerated per Django version |

Integration options:
1. **Code generation**: Generate Rust `TagSpec` code from template_linter extraction output.
2. **Runtime JSON**: Load extracted rules from JSON at djls startup.
3. **Hybrid**: Generate code for builtins, load JSON for third-party libraries.

The extracted `TagValidation` and `ParseBitsSpec` types map to djls concepts:
- `TagValidation.rules` → validation logic (replaces implicit TagSpec validation)
- `ParseBitsSpec` → arg structure for simple_tag/inclusion_tag
- `BlockTagSpec` → end_tag, intermediate_tags
- `FilterSpec` → filter arg counts

### Future Inspector Enhancements (Phase 6)

For type inference features, the inspector would need:
- View function signatures and their template usage
- Context variable names/types passed to `render()`
- Template-to-view mapping

This is out of scope for the Python prototype but documented here for planning.

## Files To Keep In Mind During Port

- `template_linter/src/template_linter/extraction/`
- `template_linter/src/template_linter/template_syntax/`
- `template_linter/src/template_linter/validation/`
- `template_linter/src/template_linter/resolution/load.py`
- `template_linter/src/template_linter/resolution/runtime_registry.py`
- `template_linter/corpus/*` (especially `manifest.toml`, `sync.py`, `discover.py`)
- `template_linter/tests/test_corpus_templates.py`
- `template_linter/HARDCODES.md`
- `template_linter/ROADMAP.md`

## Suggested "Don't Drift" Checks

- If adding a new feature in Python, ask:
  - "Can this be expressed as a generic AST pattern?"
  - "Will this be easy to reimplement from ruff AST?"
  - "Does this require Django runtime? If yes, is it optional/inspector-fed?"

- Prefer adding tests that would also exist in Rust:
  - corpus-driven tests
  - golden JSON inventories (tags/filters/block specs per library)
  - small focused templates for tokenizer + scoping behavior

## Port Trigger Checklist

This checklist defines a practical "pull the trigger" line: once these are done,
continuing to add features in Python is likely to increase drift rather than
reduce port risk.

### Deterministic Inputs
- [x] Corpus `manifest.toml` entries are pinned (sdists by minor line; repos by commit).
- [x] Corpus sync produces stable directory layouts suitable for Rust to consume.

### Golden Artifacts (Acceptance Tests for Rust)
- [x] Tokenization parity fixtures exist:
  - [x] small templates that cover `{% %}`, `{{ }}`, quoting, `{% comment %}`, `{% verbatim %}`, nested blocks, and line-number behavior
  - [x] the Python prototype can dump a canonical token stream representation
- [x] Extraction parity fixtures exist:
  - [x] per-module exports: discovered tag names + filter names
  - [x] extracted block specs (start/middle/end)
  - [x] extracted opaque blocks
  - [x] a stable JSON schema for these artifacts
- [x] End-to-end validation fixtures exist:
  - [x] a small set of templates with expected diagnostics (including strict unknowns + load scoping)
- [x] Expression parsing fixtures exist:
  - [x] valid `{% if %}` expressions (operators, literals)
  - [x] invalid expressions with expected error messages
  - [x] corpus coverage (no false positives on real templates)

### Policy Decisions Written Down
- [x] Unknown handling policy is explicit:
  - [x] unknown tags/filters/libraries in static-only mode (no registry) is best-effort and may be noisy
  - [x] unknown behavior in runtime-registry-fed mode (djls/inspector) is the "real" mode
- [x] Collision policy is explicit:
  - [x] **Not a product concern**: in the intended integration, djls provides a resolved registry
    (Django's own `Engine.libraries`/builtins ordering), so collisions are handled by Django.
  - [x] Static-only mode remains conservative and may omit ambiguous symbols; its behavior is only
    to support corpus/POC testing and should not block the port.
  - [x] "later wins" behavior for `{% load %}` and builtins is preserved (matches Django semantics).
- [x] Filter scoping policy is explicit:
  - [x] keep "final filter set" behavior for parity, or make it position-aware (intentional change)

Policy notes:
- **Unknowns (static-only mode):** without a runtime registry (installed-app ordering + builtins),
  strict unknown reporting will produce false positives in real projects. Static-only is intended
  to be conservative (avoid validating ambiguous collisions) and primarily supports corpus/POC testing.
- **Unknowns (runtime-registry-fed mode):** when djls/inspector provides the resolved mapping and
  configured builtins, strict unknowns become meaningful and should be treated as the "real" behavior.
- **Filter scoping:** keep the current "final filter set" behavior for the Python prototype to
  avoid introducing ordering-sensitive tokenization requirements right before the Rust port. If the
  Rust tokenizer preserves variable-token ordering, consider upgrading to position-aware filter scoping
  as an intentional change (with parity fixtures updated).

### Expression Parsing (Phase 4.7)
- [x] `{% if %}` / `{% elif %}` expression validation implemented:
  - [x] operators: `and`, `or`, `not`, `in`, `not in`, `is`, `is not`, `==`, `!=`, `<`, `>`, `<=`, `>=`
  - [x] literals, variables (operands treated opaquely)
  - [x] clear error messages for syntax errors
- [x] Integrated into tag validation pipeline
- [x] Tested against corpus (no false positives)
- [x] Golden fixtures for Rust parity

### Runtime Registry (Optional, But Practical for Real Projects)
- [x] A stable runtime registry JSON format exists and is documented:
  - [x] `{ "libraries": { "<load_name>": "<module.path>" }, "builtins": ["<module.path>", ...] }`
- [x] Corpus strict mode can consume `.runtime_registry.json` when present.

### "Stop Adding Python Features" Line
- [x] If the above sections are complete *and* Phase 4 corpus coverage is complete:
  - [x] proceed with Rust implementation and use the golden artifacts as the gate
  - [x] only extend Python to update goldens or clarify behavior/policy

## What NOT To Do in Python (Defer to Rust)

The following features should NOT be implemented in the Python prototype:
- Template-wide state tracking (cycle names, partial definitions)
- Variable binding tracking (`{% with %}`, `{% for %}`, `as var`)
- Type inference for template variables
- View/context introspection integration

Rationale:
- These require semantic analysis infrastructure that djls already has (`djls-semantic`).
- Implementing in Python would create API drift and duplicate effort.
- The Rust side needs these features to integrate with the rest of the LSP.

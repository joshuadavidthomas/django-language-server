# Template Linter Roadmap

This roadmap captures the planned work to reach full static validation coverage
for Django template tags and filters, then expand to third-party libraries and
real-world projects/templates.

## Phase 0: Pre-Port Cleanup (Complete)
Goal: reduce port risk by isolating "portable core" modules and removing
duplicated parsing/validation paths, without changing behavior.

Deliverables:
- Establish a strict, portable module layout:
  - `src/template_linter/extraction/`
  - `src/template_linter/template_syntax/`
  - `src/template_linter/validation/`
  - `src/template_linter/resolution/`
- Keep goldens and corpus tests green as a port acceptance harness.

## Phase 1: Django Coverage Inventory (Complete)
Goal: prove we discover every built-in Django tag and filter and identify gaps in validation.

Deliverables:
- Tag and filter inventory tests:
  - Enumerate all `@register.tag`/`register.tag(...)` and `@register.filter`/`register.filter(...)` uses.
  - Fail if any tag/filter is missing.
  - Emit JSON reports to `reports/inventory.json`.
- Opaque block behavior tests:
  - Validate that detected opaque tags are honored by parsing.
  - Verify verbatim-like behavior via detection rather than hardcoding.

## Phase 2: Tag/Filter Case Coverage (Complete)
Goal: ensure every Django tag and filter has at least one test case.

Deliverables:
- Baseline test cases:
  - One valid usage per tag.
  - One valid usage per filter.
  - Auto-generated coverage for parse_bits tags and opaque blocks.
- Real-template corpus smoke test:
  - Validate Django-shipped templates under `django/contrib/**/templates` and `django/forms/**/templates`.
  - Run with `report_unknown_tags=True` using extracted block delimiter specs (see Phase 3).
- Coverage reporting:
  - Emit JSON report to `reports/case_coverage.json`.
  - Track remaining gaps as explicit warnings.

## Phase 3: Semantic Depth + Edge Coverage (Complete For POC)
Goal: deepen validation for complex tags and cover tricky edge cases.

Targets:
- i18n tags (`blocktranslate`, `translate`, `language`), `load`, `ifchanged`.
- Tags that use parser state, token-type checks, or complex option loops.
- Filter argument coercion and edge cases (strings vs variables, numeric args).
- Block structure ("delimiter tags"): extract/validate unregistered tags like
  `endif`, `endfor`, `endblock`, `else`, `elif`, `empty` based on Django source.

Deliverables (done):
- Block delimiter extraction + structural validation:
  - Extract `parser.parse((...))` stop tokens per registered block tag.
  - Validate delimiter placement/nesting via a stack discipline check.
  - Infer a minimal delimiter ordering model (repeatable + terminal delimiters) from source patterns.
  - Infer end-tag suffix matching where supported (e.g. `{% endblock name %}`).
- Enable strict `report_unknown_tags=True` for the real-template corpus test (Django core templates).
- Focused tests for complex tags where the patterns were extractable.
- Document non-static constraints and keep them explicitly out of scope.

Deferred / out of scope for this prototype (OK to handle post-port):
- Render-time semantics that require runtime state (e.g. `ifchanged` behavior).

## Phase 4: Third-Party + Project Corpus (Complete)
Goal: expand static extraction/validation beyond Django using a large real-world
corpus (libraries + projects).

Targets (initial, pinned in `template_linter/corpus/manifest.toml`):
- django-crispy-forms
- django-debug-toolbar
- django-allauth
- django-compressor
- django-fastdev
- django-permission
- django-widget-tweaks
- horizon
- wagtail (multi-version: 7.3, 7.2, 7.1, 7.0, 6.4)
- django itself (multi-version: 6.0, 5.2, 5.1, 4.2)

Targets (initial projects, pinned as git repos):
- getsentry/sentry
- sissbruecker/linkding
- rafalp/Misago
- pretix/pretix
- django-oscar/django-oscar
- django-cms/django-cms
- netbox-community/netbox
- inventree/InvenTree
- GeoNode/geonode
- samuelclay/NewsBlur
- babybuddy/babybuddy
- ArchiveBox/ArchiveBox
- unfoldadmin/django-unfold

Targets (later):
- Additional high-signal packages discovered via Grep MCP / grep.app searches.
- "Weird" tag libraries that use uncommon parsing patterns (to harden extraction).

Deliverables:
- Local corpus harness:
  - Pinned sdist downloads into `template_linter/.corpus/` (gitignored).
  - Pinned git repo snapshots into `template_linter/.corpus/` (gitignored).
  - Corpus smoke tests that extraction succeeds across third-party templatetag modules.
- Corpus template validation:
  - Sampled validation over real templates extracted from the corpus (parametrized per file).
  - Full-corpus validation available as an opt-in slow test (`--corpus-templates-full`).
  - Entry-local extraction: validate templates using rules extracted from the entry's own `templatetags/**/*.py` (not just Django built-ins).
- Inventory tests for each library's tags/filters (per-package reports).
- Baseline valid usage tests for each tag/filter (per-package).
- Structural ("middle tag") extraction for third-party libraries:
  - Identify delimiter/middle/end tags derived from third-party sources.
  - Validate delimiter ordering/nesting in third-party templates when possible.

Status note:
- The corpus harness and default sample validation run as part of the test suite.
- Strict unknowns in corpus templates are only expected to be clean when an entry
  provides a `.runtime_registry.json` (or when djls provides an equivalent runtime
  registry); otherwise false positives are expected for project-configured builtins.
- Full-corpus strict mode is available via `--corpus-sample-per-entry=0`.

## Phase 4.5: Static `{% load %}` Resolution (Complete)
Goal: mirror Django's library resolution well enough to safely resolve collisions
between multiple `templatetags` modules within a corpus entry.

Deliverables (done):
- Build a static "library index": `library_name -> (tags, filters)` by scanning
  `templatetags/*.py` module names and registrations.
- Parse `{% load ... %}` per template and scope tag/filter validation to the
  libraries actually loaded (plus Django built-ins).
- Enable stricter unknown-tag/filter diagnostics for corpus templates without
  relying on runtime settings.

## Phase 4.6: Strict Unknowns + Runtime Registry Emulation (Complete)
Goal: enable strict unknown-tag/filter diagnostics in corpus templates without
introducing false positives, by combining:
- static `{% load %}` scoping
- an optional runtime-discovered registry (like django-language-server's inspector)

Deliverables (done):
- Strict unknowns (static-only):
  - Upgrade corpus template validation to run per-template with library scoping.
  - Enable strict unknown tags/filters in corpus validation when scoping is active.
  - Add pytest CLI flags (not env vars) to control strictness and corpus behavior.
- Runtime registry (optional, still AST-only validation):
  - Support a `.runtime_registry.json` file per corpus entry:
    - `{ "libraries": { "<load_name>": "<module.path>" }, "builtins": ["<module.path>", ...] }`
  - Use the runtime registry to:
    - resolve `{% load %}` collisions like Django (installed-app ordering)
    - incorporate project-configured builtins (tags/filters available without `{% load %}`)
  - Keep all extraction static by resolving module -> source file paths and parsing them.
- Add focused unit tests for:
  - `{% load %}` semantics and scoping boundaries
  - collisions and resolution order consistent with Django
  - `load ... from ...` selective imports (where supported)
  - runtime-registry builtins behavior (later builtins override earlier ones)

## Phase 4.7: Expression Parsing for `{% if %}` / `{% elif %}` (Complete)
Goal: validate expression syntax inside conditional tags, catching compile-time
errors that Django's `IfParser` (smartif.py) raises.

Background - Django template exceptions fall into categories:
1. **Tag compile-time** (in `do_xxx` functions): token counts, keyword positions,
   option validation - WE EXTRACT THESE via AST analysis.
2. **Expression compile-time** (in `IfParser`): operator/operand syntax errors -
   NOW HANDLED (Phase 4.7).
3. **Parser-state-dependent** (compile-time but cross-tag): cycle name existence,
   partial definitions - DEFERRED to post-port (Phase 6 scope).
4. **Render-time** (in `Node.render`): variable resolution, type coercion -
   OUT OF SCOPE (requires runtime context).

Errors this phase will catch:
- `{% if and x %}` - operator in prefix position where operand expected
- `{% if x == %}` - missing right operand
- `{% if x y %}` - missing operator between operands
- `{% if not %}` - dangling unary operator
- `{% if x in %}` - incomplete membership test

Deliverables:
- Implement expression token parser:
  - Port/adapt Django's `smartif.py` `IfParser` logic for static validation.
  - Handle operators: `and`, `or`, `not`, `in`, `not in`, `is`, `is not`, `==`, `!=`, `<`, `>`, `<=`, `>=`.
  - Handle literals and variables (operands are treated opaquely).
- Integrate with tag validation:
  - Run expression validation on `{% if %}` and `{% elif %}` tag tokens.
  - Emit clear error messages matching Django's error style.
- Test against corpus:
  - Ensure no false positives on real-world templates.
  - Add regression tests for known-bad expression patterns.

Non-goals (this phase):
- Type checking within expressions (e.g., comparing string to int).
- Validating that variables exist (requires template-wide or runtime context).
  - (Related) Operand parsing errors raised by Django's `compile_filter()` such as
    `1>2` (missing spaces) or parentheses like `(x)` are out of scope here.

## Phase 5: Static Validation of Real Templates (Complete)
Goal: validate template files or directories without runtime execution.

**Status:** Largely complete via corpus test infrastructure. No separate CLI needed -
djls will provide the user-facing interface. The prototype proves validation works.

Deliverables (done):
- Template validation via test suite (corpus template tests).
- Unknown tag/filter diagnostics with `{% load %}` scoping.
- Registry mode via `.runtime_registry.json` support.
- Load resolution and library mapping (Phase 4.5, 4.6).

Not in scope for prototype:
- Standalone CLI/API (djls provides this).
- CI-specific report formats (test suite output suffices for validation).

## Guiding Principles
- No runtime evaluation of Django templates.
- Use Django's lexer/parser for tokenization where safe, but rely on static extraction.
- Prefer explicit, actionable gap reports over silent skips.
- Keep any unavoidable hard-coded behavior centralized and overrideable (see `template_linter/src/template_linter/overrides.py`).
- Prefer extracting structural (block-aware) validation rules from Django source (see `template_linter/src/template_linter/extraction/structural.py`) over hard-coding tag names.
- Maximize *static* value for editor/LSP use-cases:
  - crisp syntax errors (tags/filters/blocks/loads) are the primary goal
  - best-effort semantics are allowed when they are derived statically and clearly labeled as such
  - runtime-only truth (request/db/render-time values) is out of scope

## Phase 6: Static Symbol + Type/Evidence Inference (Deferred to Post-Port)
Goal: provide additional language-server value without relying on runtime state.

**Note:** This phase is explicitly deferred to Rust implementation in django-language-server.
Implementing in Python would duplicate work and risk API drift. The Rust side already has
infrastructure for semantic analysis (`djls-semantic`) that this work should build upon.

Scope:
- Symbol availability:
  - tag/filter availability in scope at a position (builtins + `{% load %}` scoping)
  - optional runtime registry input to mirror installed-app ordering + project builtins
- Template-local bindings:
  - `{% with %}` and `{% for %}` variable introductions
  - tags with `as var` assignment semantics
- Template-wide state tracking:
  - `{% cycle ... as name %}` definitions available for `{% resetcycle name %}`
  - `{% partialdef name %}` definitions available for `{% partial name %}`
  - These are parser-state-dependent checks Django does at compile time, but require
    template-wide analysis rather than single-tag extraction.
- Evidence-based "types" for variables:
  - treat variable types as *best-effort evidence*, not guarantees
  - support unknowns and unions; attach confidence levels (hint vs warning vs error)
- View/context integration (requires djls inspector enhancements):
  - infer variable types from view function signatures
  - map templates to views that render them
  - provide type hints for variables that can be traced to view context

Non-goals:
- Proving runtime truth for arbitrary template variables (request/context/db-derived values).
- Validating conditions that require render-time state (e.g. `ifchanged` behavior).
- Implementing in Python prototype (do this work in Rust).

## Exception Coverage Analysis

Django template tags raise `TemplateSyntaxError` in several contexts. This section
documents what we extract/validate vs what's out of scope.

### What We Extract and Validate

| Category | Location | Example | Status |
|----------|----------|---------|--------|
| Token count checks | `do_xxx()` | `len(bits) < 4` | ✅ Extracted |
| Keyword position checks | `do_xxx()` | `bits[2] != "as"` | ✅ Extracted |
| Option validation | `do_xxx()` while loops | unknown option, duplicates | ✅ Extracted |
| parse_bits validation | `library.py` | missing required args | ✅ Via ParseBitsSpec |
| Filter arg counts | `@register.filter` | wrong number of args | ✅ Via FilterSpec |
| Block structure | `parser.parse((...))` | unclosed tags, wrong nesting | ✅ Via BlockTagSpec |
| Expression syntax | `IfParser` | `{% if and x %}` | ✅ |

### What's Deferred to Post-Port (Phase 6)

| Category | Location | Example | Why Deferred |
|----------|----------|---------|--------------|
| Cycle name tracking | `parser._named_cycle_nodes` | `{% resetcycle foo %}` | Template-wide state |
| Partial tracking | `parser._partial_defs` | `{% partial foo %}` | Template-wide state |
| with/for bindings | Block scope | Variable introductions | Semantic analysis |
| Type inference | Views + filters | Variable types | Requires djls integration |

### What's Out of Scope (Runtime)

| Category | Location | Example | Why Out of Scope |
|----------|----------|---------|------------------|
| Variable resolution | `Node.render()` | Variable doesn't exist | Runtime context |
| Type coercion | `Node.render()` | `widthratio` needs number | Runtime values |
| Format string errors | `Node.render()` | `blocktranslate` formatting | Runtime values |
| Missing partials | `PartialNode.render()` | Partial not in mapping | Runtime state |

## Pre-Port Checklist

Before porting to Rust, complete these items:

- [x] Phase 4 corpus coverage complete (strict unknowns clean with runtime registries for pinned entries)
- [x] Phase 4.7 expression parsing implemented and tested
- [x] Phase 5 real template validation (complete via corpus test infrastructure)
- [x] Policy decisions documented (see PORTING.md)
- [x] Golden artifacts finalized for Rust parity testing
- [x] No new Python features after this point (only bug fixes and golden updates)

## Reference Docs
- `.agent/research/**` contains the research narrative and spike writeups.
- `.agent/plans/**` captures historical planning artifacts.
- `.agent/progress/**` tracks long-form progress notes.

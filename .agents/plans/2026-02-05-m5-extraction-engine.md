# M5: Extraction Engine (Rules Enrichment) Implementation Plan

## Overview

Implement Rust-side rule mining using Ruff AST to derive validation semantics from Python template tag/filter registration modules. This enriches the inspector inventory with validation rules (argument counts, block structure, option constraints) that power M6 rule evaluation.

**Key architectural principles:**

1. Python inspector provides **authoritative inventory** (what exists + provenance + registry)
2. Rust does **AST extraction** (validation semantics) — no Python AST analysis in inspector
3. Salsa inputs stay minimal: `File` + `Project` only — no new global inputs
4. Extraction results keyed by `SymbolKey` to avoid collisions across libraries

**Critical extraction constraints (non-negotiable):**

1. **No `end*` string heuristics** for end-tag inference — infer closers from control flow patterns only
2. **No hardcoded split variable name** — detect the variable bound to `token.split_contents()` dynamically
3. **Conservative fallback** — emit `None` for block specs when inference is ambiguous

## Current State Analysis

### Inspector Payload (Post-M4)

From M4, `Project.inspector_inventory` contains tags and filters with provenance, where each item carries `provenance.module` — the **registration module** where `@register.tag` is called. This is the extraction target.

### What Extraction Adds

| Existing (M1-M4)            | M5 Adds                                       |
| --------------------------- | --------------------------------------------- |
| Tag names + provenance      | Argument validation rules                     |
| Filter names + provenance   | Filter arity (arg count)                      |
| Library/builtin distinction | Block specs (end_tag, intermediate)           |
| —                           | Option constraints (known values, duplicates) |
| —                           | Opaque block specs (verbatim-like)            |

### Key Existing Files

- `crates/djls-semantic/src/templatetags/specs.rs` — `TagSpec`, `TagArg` types
- `crates/djls-semantic/src/templatetags/builtins.rs` — Handcoded specs to replace
- `crates/djls-project/inspector/queries.py` — Inspector query pattern
- `crates/djls-source/src/file.rs` — `File` Salsa input pattern
- `crates/djls-server/src/db.rs` — Salsa query patterns

## Desired End State

After M5:

1. **`djls-extraction` crate exists** with pure API: `extract_rules(source: &str) -> ExtractionResult`
2. **Registration discovery** finds `@register.tag`/`@register.filter` decorators
3. **Function context detection** identifies split-contents variable dynamically
4. **Rule extraction** derives validation conditions from TemplateSyntaxError guards
5. **Block spec extraction** infers end-tags from control flow patterns (NO string heuristics)
6. **Filter arity extraction** determines argument requirements
7. **Salsa integration** wires extraction into tracked queries with proper invalidation
8. **Small fixture golden tests** verify individual patterns
9. **Corpus/full-source tests** validate at Django + ecosystem scale

## What We're NOT Doing

- **Python-side AST analysis**: Inspector reports only inventory, not validation rules
- **New Salsa inputs**: No `ExtractedRules` input — use tracked queries over `File`
- **Type checking Python code**: Extract statically provable patterns only
- **Import tracing**: Don't follow Python imports beyond registration module
- **Immediate builtins.rs removal**: Keep as fallback; extraction enriches/overrides
- **String-based end-tag heuristics**: No `starts_with("end")` or similar name matching
- **Hardcoded variable names**: No assuming `bits` — detect from `token.split_contents()` binding
- **Guessing when uncertain**: If end-tag inference is ambiguous, return `None`

---

## Implementation Plan

### Phase 1: Create `djls-extraction` Crate with Ruff Parser

**Goal**: Set up the crate with `ruff_python_parser` as a git dependency, pinned to a specific SHA (choose a recent stable Ruff release tag, e.g., v0.9.x).

Create `crates/djls-extraction/` with:
- Public API: `extract_rules(source: &str) -> ExtractionResult`
- Types: `SymbolKey { registration_module, name, kind }`, `ExtractionResult`, `TagRule`, `FilterArity`, `BlockTagSpec`
- Use a Cargo feature gate (`parser`) so that downstream crates can depend on `djls-extraction` for types only without pulling in the Ruff parser transitively. `djls-project` depends on types only; `djls-server` enables the `parser` feature.

Add `ruff_python_parser` and `ruff_python_ast` as workspace git dependencies (SHA-pinned). Verify the parser works with a trivial smoke test.

### Phase 2: Registration Discovery

**Goal**: Find `@register.tag(...)` and `@register.filter(...)` decorators in Python source.

Implement a registry scanner that walks the AST to find:
- `@register.tag` / `@register.simple_tag` / `@register.inclusion_tag` / `@register.filter` decorators
- `register.tag("name", func)` call expressions
- Registration name (from decorator keyword arg, explicit string arg, or function name)
- The decorated/referenced function for downstream analysis

The Python prototype's `template_linter/src/template_linter/extraction/registry.py` is the behavioral reference.

### Phase 3: Function Context Detection

**Goal**: Dynamically detect the variable bound to `token.split_contents()` within a compile function.

Scan the function body for a call to `split_contents()` and track which variable the result is bound to. This variable name (commonly `bits` but could be `args`, `parts`, `tokens`, etc.) is needed for rule extraction to interpret comparisons like `len(bits) < 4`.

The Python prototype's `template_linter/src/template_linter/extraction/helpers.py` (`detect_split_var`) is the behavioral reference.

### Phase 4: Rule Extraction

**Goal**: Extract validation rules from TemplateSyntaxError guard conditions.

Walk the function body looking for `raise TemplateSyntaxError(...)` statements and extract the guard conditions that precede them:
- Token count checks: `if len(bits) < 4`
- Keyword position checks: `if bits[2] != "as"`
- Option validation: while loops checking known options, duplicates
- `parse_bits` signatures for `simple_tag`/`inclusion_tag`

Represent extracted rules as structured data (argument count constraints, required keywords at positions, known option sets).

The Python prototype's `template_linter/src/template_linter/extraction/rules.py` is the behavioral reference.

### Phase 5: Block Spec Extraction (Control-Flow Based)

**Goal**: Infer end-tags and intermediate tags from `parser.parse((...))` call patterns.

Find calls to `parser.parse()` with tuple arguments containing stop-token strings. Determine:
- Which tokens are end-tags vs intermediate tags (based on control flow — if a stop-token leads to another `parser.parse()` call, it's intermediate; if it leads to return/node construction, it's terminal)
- Dynamic end-tag patterns like `f"end{tag_name}"` (best-effort)
- Opaque block detection: `parser.skip_past(...)` patterns indicate content should not be parsed

**Non-negotiable constraints**:
- Infer closers from control flow patterns only — NEVER from string prefix matching
- When inference is ambiguous (multiple candidates, unclear control flow), return `None` for the end-tag
- The `end{tag_name}` Django convention is ONLY used as a tie-breaker among candidates already found via control flow — never invented from thin air

The Python prototype's `template_linter/src/template_linter/extraction/structural.py` and `extraction/opaque.py` are the behavioral references.

### Phase 6: Filter Arity Extraction

**Goal**: Determine filter argument requirements from function signatures.

For `@register.filter` decorated functions, inspect the function signature to determine:
- Required argument count (excluding `self` and the value parameter)
- Whether an argument is optional (has a default value)

This produces a `FilterArity` (e.g., `expects_arg: bool` or `arg_count: 0..=1`).

The Python prototype's `template_linter/src/template_linter/extraction/filters.py` is the behavioral reference.

### Phase 7: Salsa Integration

**Goal**: Wire extraction into the Salsa query system with proper invalidation.

**Workspace modules** (files under project root):
- Create a tracked query `extract_module_rules(db, file: File) -> ExtractionResult`
- File edits automatically invalidate via the `File` input → re-extraction happens naturally

**External modules** (site-packages, stdlib):
- Extract during `refresh_inspector()` (not as tracked queries — these files don't change during a session)
- Store results on `Project.extracted_external_rules` field
- Manual refresh triggers re-extraction with comparison before setter call

**Module path resolution**:
- Create a resolver that maps module paths (e.g., `django.templatetags.i18n`) to file paths using `sys_path` from the Python environment
- Classify as workspace vs external based on whether the path is under project root
- `sys_path` comes from the `python_env` inspector query, stored on `Project`

**`compute_tag_specs` update**:
- Merge extracted rules into tag specs (extraction enriches/overrides `builtins.rs` defaults)
- Workspace extraction → tracked queries → automatic invalidation
- External extraction → Project field → manual refresh invalidation

### Phase 8: Small Fixture Golden Tests

**Goal**: Fast, always-run tests that verify individual extraction patterns.

Create small Python source fixtures (inline strings or fixture files) that exercise:
- Registration discovery patterns (decorator styles, call-based registration)
- Rule extraction patterns (len checks, keyword checks, option loops)
- Block spec extraction (simple end-tag, intermediates, opaque blocks)
- Filter arity detection
- Edge cases: no split_contents call, ambiguous control flow, dynamic end-tags

Use `insta` for snapshot testing where appropriate. These tests should run in `cargo test` without any external dependencies.

### Phase 9: Corpus / Full-Source Extraction Tests

**Goal**: Scale validation against real Django and ecosystem source code.

Create test infrastructure (possibly in a `djls-corpus` crate or test module) that can:
- Point at a synced corpus directory (from `template_linter/corpus/`)
- Run extraction against all `templatetags/**/*.py` files
- Verify: no panics, meaningful yield (tag/filter counts), golden snapshots for Django versions

Gate these tests on corpus availability (auto-detect default location, skip gracefully if not present).

**Temporary parity oracle**: Optionally compare Rust extraction output against the Python prototype's output. This oracle is explicitly temporary and should be deleted after M6 parity is achieved.

---

## Testing Invariants

These invariants MUST be verified by tests:

1. **No hardcoded `bits`**: Tests use `args`, `parts`, etc. and verify extraction works
2. **Primary signals first**: End-tag inference works via control flow, not naming
3. **No `end*` heuristics**: Tests prove non-conventional closer names are found correctly
4. **Never guess**: Ambiguous cases return `None` for end-tag
5. **Never invent**: End tags are never created from thin air — only from stop-tokens found in source
6. **Convention is tie-breaker only**: `end{tag_name}` used to select among existing candidates, not to create new ones
7. **Corpus diversity**: Corpus contains non-`bits` variable names, confirming dynamic detection works

---

## Performance Considerations

- Ruff parser is fast (designed for linting entire codebases)
- Extraction cached via Salsa (workspace) or Project field (external)
- Feature-gated parser dependency keeps compile times down for crates that only need types

## References

- Charter: [`.agents/charter/2026-02-05-template-validation-port-charter.md`](../charter/2026-02-05-template-validation-port-charter.md)
- RFC: [`.agents/rfcs/2026-02-05-rfc-extraction-placement.md`](../rfcs/2026-02-05-rfc-extraction-placement.md)
- Research: [`.agents/research/2026-02-04_python-ast-parsing-rust.md`](../research/2026-02-04_python-ast-parsing-rust.md)
- Research: [`.agents/research/2026-02-04_tagspecs-flow-analysis.md`](../research/2026-02-04_tagspecs-flow-analysis.md)
- Python prototype extraction: `template_linter/src/template_linter/extraction/`

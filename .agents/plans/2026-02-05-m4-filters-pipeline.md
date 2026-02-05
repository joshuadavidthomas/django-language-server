# M4: Filters Pipeline Implementation Plan

## Overview

Implement complete filter support for Django templates:

1. **Inspector filter inventory** — Collect filters from Django with provenance (builtin vs library)
2. **Structured filter representation** — Transform `Vec<String>` → structured `Filter` type with name/arg/span
3. **Filter completions** — Completions in `{{ x| }}` context
4. **Unknown filter diagnostics** — Validate filters against inventory with load scoping
5. **Load scoping for filters** — Reuse M3 infrastructure for filter availability

This builds on M1 (payload shape with provenance), M2 (Salsa invalidation), and M3 (load scoping infrastructure).

## Current State Analysis

### Parser Representation (`crates/djls-templates/src/parser.rs`)

Filters are stored as `filters: Vec<String>` on `Node::Variable`, containing raw strings like `["default:'nothing'", "title"]`. No parsing of filter name vs argument, no individual spans per filter, no validation against known filters.

### Inspector (`crates/djls-project/inspector/queries.py`)

**Filters not collected** — Only `library.tags` is iterated, `library.filters` is ignored. No filter inventory in the payload.

### Completions (`crates/djls-ide/src/completions.rs`)

`TemplateCompletionContext::Filter { partial }` exists as a placeholder that returns empty. No detection of `{{ var|` context in `analyze_template_context()`.

### Validation (`crates/djls-semantic/`)

No filter validation — `filters` field is passed through but never validated.

## Desired End State

1. **Filter completions** appear in `{{ var|` context
2. **Unknown filter** produces S111 diagnostic
3. **Unloaded filter** produces S112 diagnostic with required library name
4. **Ambiguous filter** (multiple libraries) produces S113 diagnostic
5. **Filter scoping** respects `{% load %}` via M3 infrastructure
6. **Inspector reports filter inventory** with provenance consistent with M1's tag provenance

### Inventory Type Evolution (Breaking Change in M4)

Switch the `Project` field carrying inspector data from a tags-only shape (`TemplateTags`) to a unified inventory shape that includes both tags and filters. M1-M3 are implemented against the tags-only shape; M4 expands it.

### Filter Provenance

Filter provenance mirrors `TagProvenance` from M1 — either `Library { load_name, module }` or `Builtin { module }`. The Python inspector collects filters the same way it collects tags (iterating `library.filters.items()` in addition to `library.tags.items()`).

## What We're NOT Doing

- **Filter arity/signature validation** — That's M5/M6 scope (extraction)
- **Filter argument type checking** — Runtime concern
- **Cross-template state** — Future work
- **Safe/autoescape flags** — Not needed for basic validation

---

## Implementation Plan

### Phase 1: Inspector Filter Inventory

**Goal**: Expand the inspector to collect filters alongside tags.

Update `crates/djls-project/inspector/queries.py` to iterate `library.filters.items()` (not just `library.tags`) and return a filter inventory with the same provenance structure as tags.

Update the Rust types to include a filter inventory. Expand `TemplateTags` (or create a new unified inventory type) to hold filters alongside tags, with `FilterProvenance` mirroring `TagProvenance`.

Update the Salsa query/Project field to carry the expanded inventory.

### Phase 2: Structured Filter Representation (BREAKPOINT)

**Goal**: Transform filters from raw strings to structured data with individual spans.

**This is a wide-reaching change** — `Node::Variable` is consumed by semantic analysis, block building, IDE context detection, and snapshot tests.

Create a `Filter` type (in `djls-templates`) with:
- `name: String` — the filter name (e.g., `"default"`)
- `arg: Option<String>` — the filter argument (e.g., `"'nothing'"`)
- `span: Span` — byte span of this filter within the source

Update `parse_variable()` to parse filter strings into structured `Filter` values. Handle:
- Simple filters: `title` → `Filter { name: "title", arg: None, span }`
- Filters with args: `default:'nothing'` → `Filter { name: "default", arg: Some("'nothing'"), span }`
- Colon inside quoted args: `default:'time:12:30'` → name is `"default"`, arg is `'time:12:30'`
- Filter chains: each filter gets its own span within the variable expression

Update ALL consumers of `Node::Variable.filters` in a single pass:
- `djls-semantic/blocks/tree.rs` — `NodeView::Variable`
- `djls-semantic/blocks/builder.rs` — pattern matching
- `djls-ide/context.rs` — `OffsetContext::Variable`
- All snapshot tests that include filters

### Phase 3: Filter Completions

**Goal**: Provide filter completions in `{{ x| }}` context.

Update `analyze_template_context()` in `crates/djls-ide/src/completions.rs` to detect the `{{ var|` context — specifically, when the cursor is after a pipe character inside a variable expression. Extract the partial filter name (text after the last pipe).

Implement `generate_filter_completions()` that:
- Shows builtin filters always
- Shows library filters only if their library is loaded before cursor position (reuse M3 `LoadedLibraries`)
- Filters by partial prefix
- Returns deterministic alphabetical ordering

### Phase 4: Filter Validation with Load Scoping

**Goal**: Validate filters against inventory using M3's load scoping infrastructure.

Add diagnostic codes:
- **S111**: Unknown filter (not in any inventory)
- **S112**: Unloaded filter (known but requires `{% load X %}`)
- **S113**: Ambiguous unloaded filter (defined in multiple libraries)

Wire filter validation into the semantic analysis pipeline. For each filter in a `Node::Variable`, check:
1. Is it a builtin filter? → always valid
2. Is its library loaded before this position? → valid
3. Is it known but not loaded? → S112 (single library) or S113 (multiple)
4. Is it completely unknown? → S111

Guard: if inspector inventory is `None`, skip all filter scoping diagnostics.

---

## Testing Strategy

### Parser Tests

- Filter with argument parses correctly (name vs arg separated)
- Filter chain with mixed args
- Colon inside quoted argument doesn't split
- Each filter has correct span positions
- Empty/trailing pipe edge case
- Use `insta` snapshot tests

### Completion Tests

- `{{ value|` context detected correctly
- `{{ value|def` partial filtering works
- Builtin filters always appear
- Library filters excluded when their library isn't loaded
- Inspector unavailable shows all filters

### Validation Tests

- Unknown filter → S111
- Unloaded library filter → S112
- Filter after `{% load %}` → valid
- Builtin filter → always valid
- Inspector unavailable → no scoping diagnostics

---

## Performance Considerations

- Filter inventory cached via Salsa
- Load state from M3 reused (no recomputation)
- Filter parsing is O(n) per variable expression

## Migration Notes

### Parser Representation Change (Phase 2)

**Breaking change** to `Node::Variable` — all consumers must be updated simultaneously in one PR. Run `cargo test --all` with snapshot updates.

### Inspector Filter Query

New inspector query for filters. Returns `Option<...>` so missing data is handled gracefully.

## References

- Charter: [`.agents/charter/2026-02-05-template-validation-port-charter.md`](../charter/2026-02-05-template-validation-port-charter.md) (Section M4)
- M1 Plan: [`.agents/plans/2026-02-05-m1-payload-library-name-fix.md`](2026-02-05-m1-payload-library-name-fix.md) (TagProvenance pattern)
- M3 Plan: [`.agents/plans/2026-02-05-m3-load-scoping.md`](2026-02-05-m3-load-scoping.md) (LoadedLibraries)
- Research: [`.agents/research/2026-02-04_template-filters-analysis.md`](../research/2026-02-04_template-filters-analysis.md)

# M1: Payload Shape + Library Name Fix Implementation Plan

## Overview

Fix the inspector payload structure to preserve Django library load-names and distinguish builtins from loadable libraries, then fix completions to show correct library names for `{% load %}`.

## Current State Analysis

### Python Inspector (`crates/djls-project/inspector/queries.py`)

- `TemplateTag` dataclass has `name`, `module`, `doc`
- `get_installed_templatetags()` iterates `engine.libraries.values()`, **losing the library name key**
- `module` field stores `tag_func.__module__` (defining module), not the library module
- No distinction between builtins and loadable libraries—all flattened into one list

### Rust Types (`crates/djls-project/src/django.rs`)

- `TemplateTag` struct mirrors Python: `name`, `module`, `doc`
- `module()` returns the defining module path (e.g., `django.template.defaulttags`)
- No concept of library load-name or provenance

### Completions (`crates/djls-ide/src/completions.rs`)

- `generate_library_completions()` at line 526 collects `tag.module()` as library names
- Results in completions like `django.templatetags.static` instead of `static`
- Cannot filter out builtins (which shouldn't appear in `{% load %}` completions)

## Desired End State

Per charter section 1.1:

1. **Inventory items carry:**
    - `name` — tag name as used in templates
    - `provenance` — **exactly one of:**
        - `Library { load_name, module }` — requires `{% load X %}`
        - `Builtin { module }` — always available
    - `defining_module` — where the function is defined (`tag_func.__module__`)
    - `doc` — optional docstring
2. **`{% load %}` completions show library load-names** (`static`, `i18n`) not module paths
3. **Builtins excluded from `{% load %}` completions** (they're always available)

## What We're NOT Doing

- **M3 load scoping**: Unknown tag diagnostics remain silent (pre-M3 behavior)
- **Filter inventory**: Filter collection is M4 scope
- **Collision handling**: Per charter, no collision detection in M1
- **Salsa invalidation fixes**: That's M2 scope

## Implementation Approach

Single PR with three components:

1. Expand Python inspector payload with new data model
2. Update Rust types to deserialize new payload
3. Fix completions to use `load_name` from `Library` provenance

---

## Phase 1: Python Inspector Payload Changes

### Goal

Update the inspector to return library information with proper provenance distinction, plus top-level registry structures for downstream use.

### Changes Required

**File**: `crates/djls-project/inspector/queries.py`

Update the `TemplateTag` dataclass and `TemplateTagQueryData` to carry:

- A `provenance` field that is an externally-tagged dict — either `{"library": {"load_name": str, "module": str}}` or `{"builtin": {"module": str}}`. Use a plain dict (not a dataclass) for provenance so that `asdict()` produces the externally-tagged union shape that Rust's serde expects.
- A `defining_module` field for `tag_func.__module__`
- Top-level `libraries: dict[str, str]` (load_name → module_path from `engine.libraries`)
- Top-level `builtins: list[str]` (ordered builtin module paths from `engine.builtins`)

Rewrite `get_installed_templatetags()` to:

- Iterate `engine.template_builtins` paired with `engine.builtins` (via `zip`) to collect builtin tags with `Builtin` provenance. Guard against length mismatch between these two lists.
- Iterate `engine.libraries.items()` (not `.values()`) to preserve load-name keys, collecting library tags with `Library` provenance.
- Return the top-level `libraries` and `builtins` mappings alongside the tag inventory.

### Success Criteria

- Inspector payload includes top-level `libraries` dict with load-names as keys
- Inspector payload includes top-level `builtins` list in correct order
- Builtin provenance modules are correct (e.g., `django.template.defaulttags`, not `django.template.library`)
- Library provenance has both `load_name` and `module` fields
- `cargo build` passes (which exercises inspector via tests)

---

## Phase 2: Rust Type Updates

### Goal

Update Rust types to deserialize the new payload structure with a `TagProvenance` enum and top-level registry data.

### Changes Required

**File**: `crates/djls-project/src/django.rs`

- Add a `TagProvenance` enum with `Library { load_name, module }` and `Builtin { module }` variants. Use `#[serde(rename_all = "lowercase")]` for externally-tagged deserialization matching the Python dict shape.
- Update `TemplateTag` to carry `provenance: TagProvenance` and `defining_module: String` instead of the current single `module` field.
- Add clear accessors that distinguish between:
  - `defining_module()` — where the function is defined (the old `module()` behavior)
  - `registration_module()` — the library/builtin module where it's registered
  - `library_load_name()` — returns `Some(load_name)` for library tags, `None` for builtins
  - `is_builtin()` — convenience predicate
- Expand `TemplateTags` to hold top-level `libraries: HashMap<String, String>` and `builtins: Vec<String>` alongside the tag inventory, with appropriate accessors.
- Derive `PartialEq` and `Eq` where needed for downstream comparison (M2 depends on this).
- Update the `templatetags` Salsa query to construct `TemplateTags` from the expanded response.
- Export `TagProvenance` from `crates/djls-project/src/lib.rs`.

### Tests

- Verify `TagProvenance` enum deserializes correctly from JSON (both library and builtin variants)
- Verify accessor methods work for both provenance types
- Verify `TemplateTags` registry data accessors return expected data

### Success Criteria

- `cargo build -p djls-project` passes
- `cargo clippy -p djls-project --all-targets -- -D warnings` passes
- Unit tests pass for deserialization and accessors

---

## Phase 3: Completions Fix

### Goal

Update completions to use library load-name and exclude builtins from `{% load %}` completions.

### Changes Required

**File**: `crates/djls-ide/src/completions.rs`

- Fix `generate_library_completions()` to use `tags.libraries()` keys directly instead of extracting module paths from individual tags. Sort library names alphabetically for deterministic completion ordering (HashMap iteration is nondeterministic).
- Update tag name completion detail text to show useful provenance info: library tags should show the library load-name and a `{% load X %}` hint; builtin tags should show "builtin from {module}".
- Ensure tag iteration works with the updated `TemplateTags` type (it no longer implements `Deref` but has an `iter()` method).

### Tests

- Library completions show library names, not module paths
- Library completions are in deterministic alphabetical order
- Partial prefix filtering works
- Builtin tags excluded from library completions

### Success Criteria

- Full build passes: `cargo build`
- All tests pass: `cargo test`

---

## Migration Notes

This is a **breaking change** to the inspector payload format. The old flat `{name, module, doc}` shape becomes `{name, provenance, defining_module, doc}` with top-level `libraries` and `builtins` fields. No data migration needed — this is runtime data from Django introspection.

## References

- Charter: [`.agents/charter/2026-02-05-template-validation-port-charter.md`](../charter/2026-02-05-template-validation-port-charter.md)
- Current inspector: `crates/djls-project/inspector/queries.py`
- Current Rust types: `crates/djls-project/src/django.rs`
- Current completions: `crates/djls-ide/src/completions.rs`

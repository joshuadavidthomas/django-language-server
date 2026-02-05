# M3: Load Scoping Infrastructure Implementation Plan

## Overview

Implement position-aware `{% load %}` scoping for tags and filters, enabling:

- **Diagnostics**: "Tag `{% trans %}` requires `{% load i18n %}`" when a tag is used before its library is loaded
- **Completions**: Only show tags available at cursor position (builtins + libraries loaded before cursor)
- **Selective imports**: `{% load trans from i18n %}` correctly scopes only the named symbols

This builds on M1 (payload shape with `TagProvenance`) and M2 (Salsa invalidation plumbing).

## Current State Analysis

### Parser Representation of `{% load %}`

`{% load %}` is parsed as a generic `Node::Tag` in `crates/djls-templates/src/nodelist.rs` with `name: "load"` and `bits` capturing library names or selective import tokens (e.g., `["trans", "from", "i18n"]`). Lacks semantic structure.

### Unknown Tags Pass Silently

In `crates/djls-semantic/src/arguments.rs`, any tag without a `TagSpec` passes validation silently.

### Block Builder Ignores Unknown Tags for Scoping

In `crates/djls-semantic/src/blocks/builder.rs`, unknown tags are classified as `TagClass::Unknown` and added as leaf nodes without scoping consideration.

### Completions Show All Tags

In `crates/djls-ide/src/completions.rs`, all tags from `TemplateTags` are shown regardless of `{% load %}` state.

### M1 Payload Shape (Assumed Complete)

After M1, `TemplateTags` contains `libraries: HashMap<String, String>`, `builtins: Vec<String>`, and tags with `TagProvenance::Library { load_name, module }` or `TagProvenance::Builtin { module }`.

## Desired End State

### Per Charter Section 1.2

1. **Diagnostics respect load scope**: A tag from library `foo` produces an error if `{% load foo %}` hasn't preceded it
2. **Completions respect load scope**: Tag completions only show symbols available at cursor position
3. **`{% load X from Y %}` handled**: Selective imports correctly scope only the named symbols
4. **Builtins always available**: Tags from `engine.template_builtins` available without `{% load %}`

### Unknown Tag/Filter Behavior (Post-M3)

| Scenario                  | Behavior                                                               |
| ------------------------- | ---------------------------------------------------------------------- |
| **Inspector healthy**     | Unknown tags/filters produce diagnostics by default (S108, S109, S110) |
| **Inspector unavailable** | Suppress S108/S109/S110 entirely; show all tags in completions         |
| **Truly unknown**         | Error: "Unknown tag `{% xyz %}`" / "Unknown filter `\|xyz`"           |

### "Inspector Unavailable" Behavior (Explicit)

| Component                                                | Check                                | Behavior when unavailable               |
| -------------------------------------------------------- | ------------------------------------ | --------------------------------------- |
| **Validation** (`validate_tag_scoping`)                  | `db.inspector_inventory().is_none()` | Return early, emit no S108/S109/S110    |
| **Completions** (`generate_tag_name_completions`)        | `loaded_libraries.is_none()`         | Skip availability filter, show all tags |
| **Library completions** (`generate_library_completions`) | `template_tags.is_none()`            | Return empty (no libraries known)       |

## What We're NOT Doing

- **`{% extends %}`/`{% include %}` scoping**: Load scope inheritance is future work
- **Filter validation**: Filter scoping/completions is M4 scope
- **Cross-template state**: Cycle names, partialdef tracking deferred

## Correctness Requirements

### Selective Import vs Full Load Logic

Use a **state-machine approach** that processes load statements in document order:

- `fully_loaded: HashSet<load_name>` — libraries fully loaded
- `selective: HashMap<load_name, HashSet<symbol>>` — selective imports
- On `{% load X Y %}` (full load): add to `fully_loaded` AND **clear** `selective[lib]`
- On `{% load X from Y %}` (selective): if library NOT fully loaded, add symbols to `selective[library]`
- Tag available iff `library ∈ fully_loaded` OR `tag_name ∈ selective[library]`

This ensures `{% load trans from i18n %}` followed by `{% load i18n %}` correctly makes ALL i18n tags available.

### Tag-Name Collision Handling

When building the tag inventory from inspector data, track ALL candidate libraries for each tag name. When emitting errors:

- Single library candidate → S109 with specific library name ("requires `{% load X %}`")
- Multiple library candidates → S110 (AmbiguousUnloadedTag) listing all candidates

### Structural Tag Exclusion

Skip structural tags (openers/closers/intermediates that have TagSpecs) when checking load scoping — those are validated by block/argument validation, not load scoping. For example, `{% endif %}` should never get S108 even if its opener's library isn't loaded.

---

## Implementation Plan

### Phase 1: Load Statement Parsing and Data Structures

**Location**: New module `crates/djls-semantic/src/load_resolution.rs`

Create data structures for:

- **`LoadStatement`**: Captures a parsed `{% load %}` tag with its `span`, the list of library names (for full loads), and an optional selective import (symbols + library name for `{% load X from Y %}`).
- **`LoadedLibraries`**: An ordered collection of `LoadStatement` values that can answer "what libraries/symbols are available at a given position?" by filtering loads whose span ends before the query position.

Implement a `parse_load_bits` function that takes the `bits` from a `Node::Tag` with `name == "load"` and determines whether it's a full library load or a selective import (`from` keyword detection).

Export the new module from `crates/djls-semantic/src/lib.rs`.

**Tests**: Parse various load syntaxes — `{% load i18n %}`, `{% load i18n static %}`, `{% load trans from i18n %}`, `{% load trans blocktrans from i18n %}`, empty load (edge case).

### Phase 2: Compute LoadedLibraries from NodeList

Add a tracked Salsa query `compute_loaded_libraries(db, file) → LoadedLibraries` that:

- Iterates all nodes in the file's nodelist
- Identifies `Node::Tag` with `name == "load"`
- Parses each load tag's bits into `LoadStatement`
- Returns an ordered `LoadedLibraries` collection

**Tests**: Given a nodelist with load tags at various positions, verify the computed `LoadedLibraries` returns correct results.

### Phase 3: Available Symbols Query

Create an `AvailableSymbols` type that represents the set of tags available at a given position, and a query to compute it:

- **Inputs**: `LoadedLibraries` (from Phase 2), inspector inventory (from `Project`), and a cursor position
- **Logic**:
  - Start with all builtin tags (always available)
  - Add tags from fully-loaded libraries (where load span < position)
  - Add selectively-imported symbols (where load span < position)
  - Handle the state-machine semantics for selective-then-full load ordering
- **Output**: A set of available tag names plus a mapping of unavailable tags to their required library/libraries

**Tests**: Comprehensive scoping boundary tests — tag before load (unavailable), tag after load (available), selective imports, full load overriding selective, multiple libraries for same tag name, builtins always available.

### Phase 4: Validation Integration — Unknown Tag Diagnostics

Wire the available symbols query into the validation pipeline:

- Add new error variants: `S108` (UnknownTag), `S109` (UnloadedTag — requires specific library), `S110` (AmbiguousUnloadedTag — multiple candidates)
- Add the new diagnostic codes to the diagnostic system
- Extend the `SemanticDb` trait with `inspector_inventory()` so validation can check inspector health
- In tag validation, after checking TagSpecs (structural tags), check if the tag is in the available symbols set. If not, determine whether it's truly unknown (S108), known but unloaded (S109), or ambiguously defined across multiple libraries (S110).
- **Guard**: If inspector inventory is `None`, skip all S108/S109/S110 diagnostics entirely.

**Tests**: Unknown tag produces S108, unloaded library tag produces S109 with correct library name, tag from multiple libraries produces S110, inspector unavailable produces no scoping diagnostics, structural tags (closers/intermediates) skip scoping checks.

### Phase 5: Completions Integration

Update `generate_tag_name_completions` to filter tags by availability at cursor position:

- Accept `LoadedLibraries` and inspector inventory as parameters
- When inspector is available: only show builtins + tags from loaded libraries at cursor position
- When inspector is unavailable: show all tags (fallback behavior, no filtering)
- Update call sites in the server to pass the new parameters

**Tests**: Before any load only builtins appear, after `{% load i18n %}` i18n tags appear, inspector unavailable shows all tags.

### Phase 6: Library Completions Enhancement

Update `generate_library_completions` to use the inspector inventory's library data directly:

- Show all known library names from `tags.libraries()` (already done in M1)
- When inspector unavailable: return empty (no libraries known)

**Tests**: Library completions show correct names, inspector unavailable returns empty list.

---

## Testing Strategy

### Integration Test Templates

Create test templates that exercise scoping boundaries:

1. **Basic scoping**: Builtin works everywhere, library tag before load → S109, library tag after load → valid, unknown tag → S108
2. **Selective then full**: `{% load trans from i18n %}` → only trans available; then `{% load i18n %}` → all i18n tags available
3. **Collision handling**: Tag defined in multiple libraries, none loaded → S110; load one → valid

### Completion Position Tests

1. Before any load: only builtins
2. After `{% load i18n %}`: builtins + i18n tags
3. With `{% load trans from i18n %}`: builtins + trans only
4. Multiple loads: cumulative availability
5. Inspector unavailable: all tags shown (fallback)

### TagSpecs/Structural Edge Cases

1. `{% endif %}` should NOT get S108 (has spec via opener)
2. `{% else %}` should NOT get S108 (has intermediate spec)
3. `{% endfoo %}` where "foo" unknown → S108 only if no spec exists

---

## Performance Considerations

- **Single-pass extraction**: `compute_loaded_libraries` is O(n) over nodelist
- **Cached via Salsa**: LoadedLibraries computed once per file revision
- **Position lookup**: O(k) where k = number of load statements (typically small)

## Migration Notes

This is a **user-visible behavior change**: unknown tags now produce diagnostics (when inspector healthy), and completions are filtered by load state.

## References

- Charter: [`.agents/charter/2026-02-05-template-validation-port-charter.md`](../charter/2026-02-05-template-validation-port-charter.md) (Section 1.2)
- M1 Plan: [`.agents/plans/2026-02-05-m1-payload-library-name-fix.md`](2026-02-05-m1-payload-library-name-fix.md) (TagProvenance)
- M2 Plan: [`.agents/plans/2026-02-05-m2-salsa-invalidation-plumbing.md`](2026-02-05-m2-salsa-invalidation-plumbing.md) (inspector_inventory)
- Research: [`.agents/research/2026-02-04_load-tag-library-scoping.md`](../research/2026-02-04_load-tag-library-scoping.md)

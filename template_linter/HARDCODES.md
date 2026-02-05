# Hard-Coded Behavior Inventory

This project aims to statically validate Django template syntax based on the
Python source for tags/filters. In practice, a small number of behaviors are
hard to derive purely from AST patterns (or would be too fragile).

This document inventories the current hard-coded behavior and points to the
central override location.

## Central Override Location
- `template_linter/src/template_linter/overrides.py`

## Current Hard-Coded Items

### Token-List Variable Names (Validation)
Why: Some extracted rules compare `len(x)` where `x` is intended to be the raw
token list (e.g., `bits = token.split_contents()`).

Risk: Treating generic names like `args` as "token lists" creates false
positives for third-party tags that use `args/kwargs` to mean parsed positional
and keyword arguments (not raw tokens).

Status:
- `template_linter/src/template_linter/overrides.py` contains `TOKEN_LIST_VARS`.
- It is intentionally minimal (`{"bits"}`).
- When a variable is actually derived from `token.split_contents()`, extraction
  records it in the per-rule `TokenEnv`, and validation resolves it without
  needing this allowlist.

### Parsing Defaults (Opaque Blocks)
Why: If a caller does not pass extracted opaque blocks, parsing won't be able to
skip inner tokens for opaque tags.

Status: We support extraction of opaque blocks from Django source via
`extract_opaque_blocks_from_django()`, and tests use that. The library does not
automatically apply any hard-coded opaque-block defaults.

`template_linter/src/template_linter/overrides.py` includes
`DEFAULT_OPAQUE_BLOCKS`, but it is currently empty and exists only as a
centralized extension point.

### Extraction Heuristic Overrides
Why: Some compile function names don't map cleanly to tag names via naming
convention alone.

None currently (goal is to rely on actual registrations rather than naming).

### Validation-time Tag Aliases
Why: A fallback when rules exist under a canonical name but templates use an
alias. Ideally extraction should emit rules under all registered names.

Status: Extraction should emit rules under all registered names. The optional
`TAG_ALIASES` extension point exists in `template_linter/src/template_linter/overrides.py`
but is currently empty and is not enabled by default.

### Structural (Block-aware) Validation Rules
Why: Some tags enforce constraints on inner block tags (not just the opening
tag syntax). This is still static (template scan) but is derived from
AST-based extraction.

Structural rules should be extracted from Django source via:
- `template_linter/src/template_linter/extraction/structural.py`

Hard-coded fallback rules (if needed) can live in `overrides.py` via the
`DEFAULT_STRUCTURAL_RULES` extension point, but callers should prefer passing
extracted `structural_rules` into `validate_template`.

## Note On API Shape
Hard-coded behavior is currently consumed via explicit keyword arguments on
public APIs (e.g., `validate_template(..., structural_rules=...)`) rather than a
config object.

## Extension Guidance
If you need to add (or remove) an escape hatch, prefer:
1. Add the data/override to `overrides.py`.
2. Keep core extraction/validation generic.
3. Add a focused test that demonstrates why the override exists.

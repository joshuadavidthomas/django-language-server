# CodeRabbit Review Plan — Round 2

## ✅ ACCEPT — Fix These

### 1. `completions.rs:688,709,750,800` — PartialClose appends `" %"` instead of `" %}"`
- **Severity**: Medium (user-facing bug)
- **File**: `crates/djls-ide/src/completions.rs`
- **Fix**: Four match arms for `ClosingBrace::PartialClose` append `" %"` instead of `" %}"`. The replacement range (from `calculate_replacement_range`) extends past the auto-paired `}`, consuming it. But the insert text doesn't re-add it, leaving the user with `{% tag argument %` (missing closing `}`). The correct behavior is shown at line 906 in `build_plain_insert_for_tag`, which handles `PartialClose | None` together and appends `" %}"` with the comment "Include full closing since we're replacing the auto-paired }". Fix all four locations (688, 709, 750, 800) to append `" %}"`.
- **Note**: CodeRabbit reported this as two separate findings (#16 and #17 in the raw output) — they are the same bug in multiple locations.

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: crates/djls-ide/src/completions.rs | **Lines**: 686 to 690 / 707 to 711
>
> In @crates/djls-ide/src/completions.rs around lines 686 - 690, In the argument completion branch where you build insert_text, change the PartialClose case to append " %}" instead of " %" so it matches calculate_replacement_range behavior and mirrors build_plain_insert_for_tag.

</details>

### 2. `effects.rs:205-232` — `extract_option_equality` doesn't handle reversed comparisons
- **Severity**: Medium (missed analysis)
- **File**: `crates/djls-extraction/src/dataflow/eval/effects.rs`
- **Fix**: Only handles `option == "value"` (variable on left). Should also handle `"value" == option` (literal on left, Yoda condition). Both patterns are idiomatic Python. Add a fallback branch that checks if `left` is `ExprStringLiteral` and `comparators[0]` is `ExprName` matching `option_var`.

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: crates/djls-extraction/src/dataflow/eval/effects.rs | **Lines**: 205 to 232
>
> Update extract_option_equality to also handle the reversed comparison where the left side is the string literal and the comparator is the variable.

</details>

### 3. `calls.rs:247-251` — `collect_returns` doesn't handle `Stmt::Match`
- **Severity**: Medium (missing Python 3.10+ support)
- **File**: `crates/djls-extraction/src/dataflow/calls.rs`
- **Fix**: Return statements inside `match`/`case` blocks are missed. Django 5.x requires Python 3.10+, so match statements can appear in template tag code. Add a `Stmt::Match` arm that iterates over `match_stmt.cases` and calls `collect_returns` on each case body, like how `Stmt::With` handles its body.

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: crates/djls-extraction/src/dataflow/calls.rs | **Lines**: 247 to 251
>
> The code currently ignores Stmt::Match variants so return statements inside Python 3.10+ match/case blocks are missed.

</details>

### 4. `expressions.rs:20-26` — Misleading doc comment for `eval_expr`
- **Severity**: Low (incorrect documentation)
- **File**: `crates/djls-extraction/src/dataflow/eval/expressions.rs`
- **Fix**: Doc comment says "When ctx is provided..." but `eval_expr` doesn't accept a `ctx` parameter — it always passes `None` to `eval_expr_with_ctx`. Remove or rewrite the misleading sentence to say this is a convenience wrapper that delegates to `eval_expr_with_ctx(expr, env, None)`.

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: crates/djls-extraction/src/dataflow/eval/expressions.rs | **Lines**: 20 to 26
>
> The doc comment for eval_expr is incorrect: remove the sentence about "When ctx is provided..." and replace it with a short description that this function is a convenience wrapper.

</details>

## 🟡 ACCEPT WITH CAVEATS — Worth Doing, But Lower Priority

### 5. `resolve.rs:161-174` — Non-deterministic `pythonX.Y` directory selection
- **Severity**: Low (edge case)
- **File**: `crates/djls-project/src/resolve.rs`
- **Fix**: `std::fs::read_dir()` returns entries in arbitrary filesystem order. If a venv has multiple `pythonX.Y` dirs (unlikely but possible from migration artifacts, symlink farms, etc.), the selected directory is non-deterministic. Should collect matching entries, sort by parsed version descending, and pick the highest.
- **Caveat**: In practice, venvs have exactly one `pythonX.Y` directory. This is defensive hardening, not a live bug.

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: crates/djls-project/src/resolve.rs | **Lines**: 161 to 174
>
> The loop over std::fs::read_dir(lib_dir.as_std_path()) is non-deterministic and can pick an arbitrary pythonX.Y directory.

</details>

### 6. `lib.rs:69-77` — `latest_django()` uses lexicographic version ordering
- **Severity**: Low (corpus dev tool)
- **File**: `crates/djls-corpus/src/lib.rs`
- **Fix**: `synced_children().sort()` + `.last()` uses string sort. Versions with multi-digit components sort incorrectly: `"5.2.9"` > `"5.2.10"` lexicographically. Should parse with semver and compare properly.
- **Caveat**: This is a developer tool, not production code. The current Django corpus versions (4.2, 5.0, 5.1, 5.2) all have single-digit minor versions. Patch versions like `5.2.11` vs `5.2.9` could trigger this, but only when multiple patch versions are synced simultaneously, which the corpus design doesn't really do (it syncs a single version per minor release line).

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: crates/djls-corpus/src/lib.rs | **Lines**: 69 to 77
>
> latest_django() uses synced_children(&django_dir).into_iter().last(), which relies on lexicographic ordering and can misidentify the newest semantic version.

</details>

### 7. `archive.rs:82-97` — Add absolute path rejection after stripping top-level dir
- **Severity**: Low (defense-in-depth)
- **File**: `crates/djls-corpus/src/archive.rs`
- **Fix**: After `split_once('/')`, an entry like `Django-5.2//etc/passwd` would produce `/etc/passwd`. Add a check for `is_absolute()` or `RootDir`/`Prefix` components before proceeding. Cheap defensive check.
- **Caveat**: The corpus downloads from known Django release URLs. The existing `ParentDir` check already covers path traversal. This is belt-and-suspenders.

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: crates/djls-corpus/src/archive.rs | **Lines**: 82 to 97
>
> After computing relative from entry_path, add a rejection for paths that become absolute to prevent absolute path escape after stripping the top-level directory.

</details>

### 8. `blocks.rs:566-652` — Missing `orelse` recursion in `body_has_parse_call` and `collect_skip_past_tokens`
- **Severity**: Low (theoretical)
- **File**: `crates/djls-extraction/src/blocks.rs`
- **Fix**: For/While/Try arms only recurse into `.body` but not `.orelse`. Additionally, `collect_skip_past_tokens` is missing a `Stmt::With` arm that `body_has_parse_call` has. Should add orelse handling for completeness and add the With arm.
- **Caveat**: In Django template tag compilation functions, `parser.parse()` / `parser.skip_past()` calls in `else` blocks of loops are extremely unlikely. These are narrow-domain analyzers for a very specific code pattern. No real-world Django code puts parse calls in loop else blocks.

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: crates/djls-extraction/src/blocks.rs | **Lines**: 566 to 652 / 107 to 122
>
> Both functions fail to recurse into the orelse branches for For/While/Try and collect_skip_past_tokens also omits With.

</details>

## ❌ IGNORE — Not Applicable or Wrong

### 9. `validation.rs:72-84` and `validation.rs:167-180` — `env_symbols[0]` "unsafe" access
- **Reason**: FALSE POSITIVE. The `env_symbols` vector comes from `tags_by_name()` / `filters_by_name()` which build their HashMap using `.or_default().push()`. Any key that exists in the map is guaranteed to have at least one element. The `[0]` access is guarded by `if let Some(env_symbols) = map.get(name)`, so it only executes when the key exists, meaning the vector is always non-empty.

### 10. `corpus/lib.rs:153-166` and `corpus/lib.rs:111-118` — Windows path separators in `path_str.contains()`
- **Reason**: While Windows IS a CI target, these path checks are in the **corpus sync tool** (`djls-corpus`), which processes downloaded Django tarballs. Tarballs always use forward slashes internally regardless of the host OS. The paths being checked come from tarball extraction, not from the local filesystem. Additionally, `camino::Utf8Path` constructed from tarball paths preserves forward slashes. The `/templates/` and `/templatetags/` substring checks work correctly for tarball-origin paths even on Windows.

### 11. `corpus/main.rs:31-36` — `CARGO_MANIFEST_DIR` with custom `--manifest`
- **Reason**: Same issue as Round 1 #10, already assessed as SKIP. This is a developer tool run from the repo. `CARGO_MANIFEST_DIR` is correct for finding the corpus root relative to the crate source. The `--manifest` flag overrides the manifest config, not the corpus root location.

### 12. `IMPLEMENTATION_PLAN.md:786` — M13 plan reference to non-existent file
- **Reason**: Documentation housekeeping in a planning document. The M13 milestone is complete; the plan file was never needed because the work was done directly. Not worth creating a retroactive plan file or editing the implementation plan just for this reference.

### 13. `CORPUS_REFACTOR_PLAN.md` — Title says "Plan" but everything is "✅ Done"
- **Reason**: The work IS completed. The plan document serves as a historical record of what was done and why. Renaming it doesn't add value. Having completed checkmarks in a plan is normal — it shows the plan was executed.

### 14. `lib.rs:156-177` — `SortedExtractionResult` key_str omits `SymbolKey.kind`
- **Reason**: FALSE POSITIVE. The `key_str` is used within separate maps: `tag_rules` (all Tag kind), `filter_arities` (all Filter kind), `block_specs` (all Tag kind). Within any single map, all entries have the same kind, so including kind in the key adds no uniqueness. A tag and filter with the same module+name cannot collide because they're in different maps.

### 15. `calls.rs:73` — `Tuple` mapped to `AbstractValueKey::Other`
- **Reason**: LOW VALUE. This is a cache key for function call memoization. Different tuple contents mapping to the same key means potential cache collisions, but in Django template tag code, calling the same function with different tuple arguments is extremely rare. The existing conservative behavior (treating all tuples as the same for caching purposes) errs on the side of re-evaluating rather than returning wrong results, since cache hits would just skip redundant work. Not worth the complexity of recursive key construction.

### 16. `registry.rs:378-389` — `kw_callable_name` doesn't handle `Expr::Attribute`
- **Reason**: In Django's `Library.tag()` API, keyword arguments like `node_class=MyNode` always use simple name references, not dotted attribute paths. `node_class=myapp.nodes.MyNode` is not valid Django API usage — the node class must be imported and referenced directly. No real-world Django code would trigger this.

### 17. `corpus/lib.rs:250-254` — `components[start..]` potential panic
- **Reason**: The `start` index is computed by `find_position` which searches for known corpus path markers. The function is only called on paths that have already been filtered through `extraction_targets_in` or `templates_in`, which validate path structure. If a path makes it this far without the expected markers, the empty-slice case (when `start >= len`) returns an empty string via `join(".")` — it doesn't panic. Rust slice indexing with `[start..]` where `start == len` returns an empty slice, which is valid.

## Summary

| # | File | Verdict | Priority |
|---|------|---------|----------|
| 1 | `completions.rs` PartialClose (×4) | ✅ Accept | **Medium** |
| 2 | `effects.rs` reversed comparisons | ✅ Accept | **Medium** |
| 3 | `calls.rs` Stmt::Match returns | ✅ Accept | **Medium** |
| 4 | `expressions.rs` doc comment | ✅ Accept | Low |
| 5 | `resolve.rs` pythonX.Y ordering | 🟡 Accept | Low |
| 6 | `lib.rs` version sorting | 🟡 Accept | Low |
| 7 | `archive.rs` absolute path check | 🟡 Accept | Low |
| 8 | `blocks.rs` orelse recursion | 🟡 Accept | Low |
| 9 | `validation.rs` env_symbols[0] (×2) | ❌ False positive | — |
| 10 | `lib.rs` Windows path separators (×2) | ❌ Ignore (tarball paths) | — |
| 11 | `main.rs` CARGO_MANIFEST_DIR | ❌ Ignore (repeat) | — |
| 12 | `IMPLEMENTATION_PLAN.md` M13 ref | ❌ Ignore (docs) | — |
| 13 | `CORPUS_REFACTOR_PLAN.md` title | ❌ Ignore (docs) | — |
| 14 | `lib.rs` key_str kind | ❌ False positive | — |
| 15 | `calls.rs` Tuple key | ❌ Low value | — |
| 16 | `registry.rs` Expr::Attribute | ❌ Ignore (not real API) | — |
| 17 | `lib.rs` components slice | ❌ Ignore (safe) | — |

**4 to fix (items 1–4), 4 minor improvements (items 5–8), 9 to ignore.**

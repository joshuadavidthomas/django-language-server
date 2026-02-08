# CodeRabbit Review Plan

## ✅ ACCEPT — Fix These

### 1. `archive.rs:39-67` — Tar extraction doesn't handle entry types
- **Status**: DONE
- **Severity**: High (security-adjacent)
- **File**: `crates/djls-corpus/src/archive.rs`
- **Fix**: The loop calls `read_to_end` + `write` on every entry regardless of type. Directory entries become empty files; symlinks are written as regular files containing the target path. Should check `entry.header().entry_type()` — skip directories (parent dirs are already created), reject symlinks with `bail!`, and only `read_to_end`/`write` for regular files.

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: crates/djls-corpus/src/archive.rs | **Lines**: 39 to 67 | **Type**: potential_issue
>
> In @crates/djls-corpus/src/archive.rs around lines 39 - 67, The loop over archive.entries currently treats every entry as a regular file (calls entry.read_to_end and writes), but must first inspect the entry type and handle directories and symlinks: for each entry yielded by archive.entries()? (the local variable entry from the diff), skip directory entries (do not call read_to_end or write; ensure parent dirs are created as now) and explicitly reject symlink entries by returning an error (anyhow::bail) when you detect an EntryType::Symlink (use the tar entry header/entry_type API to check the type); keep existing path-traversal check, and only call read_to_end and std::fs::write for regular file entries.

</details>

### 2. `statements.rs:110-115` — While arm never processes loop body
- **Status**: DONE
- **Severity**: High (missed analysis)
- **File**: `crates/djls-extraction/src/dataflow/eval/statements.rs`
- **Fix**: The `While` arm only calls `try_extract_option_loop` and never calls `process_statements` on `while_stmt.body`. The `Match` arm right below it shows the correct pattern. Assignments and side-effects inside while loops are silently ignored. Should add `process_statements(&while_stmt.body, env, ctx)` after the option-loop extraction.

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: crates/djls-extraction/src/dataflow/eval/statements.rs | **Lines**: 110 to 115 | **Type**: potential_issue
>
> In @crates/djls-extraction/src/dataflow/eval/statements.rs around lines 110 - 115, The While arm only extracts an option-loop pattern and never processes the loop body, so side-effects and assignments inside the loop are missed; update the Stmt::While handler (the branch matching while_stmt and calling try_extract_option_loop) to always call process_statements on while_stmt.body (using the existing process_statements function with env and ctx) after/alongside setting ctx.known_options so the loop body is analyzed whether or not try_extract_option_loop returns Some.

</details>

### 3. `types.rs:71-87` — `debug_assert_eq!` in `rekey_module` won't fire in release
- **Status**: TODO
- **Severity**: Medium (silent data loss in release)
- **File**: `crates/djls-extraction/src/types.rs`
- **Fix**: If rekeying causes key collisions, `map.extend` silently drops entries. The `debug_assert_eq!` only catches this in debug builds. Should promote to `assert_eq!` or use `tracing::error!` + return `Result`.

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: crates/djls-extraction/src/types.rs | **Lines**: 71 to 87 | **Type**: potential_issue
>
> In @crates/djls-extraction/src/types.rs around lines 71 - 87, The current rekey_module inner function uses debug_assert_eq!, which only runs in debug builds and can silently drop entries in release; change that check to run in all builds by replacing debug_assert_eq! with assert_eq! (or otherwise perform an explicit length check and panic via panic! with the same message), referring to rekey_module -> rekey_map, SymbolKey.registration_module, and the FxHashMap so the assertion triggers in release and prevents silent data loss; alternatively, if you prefer not to panic, implement an explicit duplicate-detection step before map.extend and either return a Result from rekey_module or emit a tracing::warn! with details about the conflicting keys.

</details>

### 4. `match_arms.rs:54-60` — Wildcard doesn't unconditionally set min to 0
- **Status**: TODO
- **Severity**: Medium (incorrect constraint calculation)
- **File**: `crates/djls-extraction/src/dataflow/eval/match_arms.rs`
- **Fix**: If a `Variable { min_len: 2 }` arm is processed before `Wildcard`, `min_variable_length` stays at `Some(2)`. But a wildcard matches anything (including zero-length), so the overall minimum should be 0. The `if min_variable_length.is_none()` guard should be removed — wildcard should unconditionally set `min_variable_length = Some(0)`.

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: crates/djls-extraction/src/dataflow/eval/match_arms.rs | **Lines**: 54 to 60 | **Type**: potential_issue
>
> In @crates/djls-extraction/src/dataflow/eval/match_arms.rs around lines 54 - 60, The Wildcard branch in PatternShape::Wildcard must force the minimum variable length to zero unconditionally; update the handling in match_arms.rs so that when PatternShape::Wildcard is encountered you set has_variable_length = true and set min_variable_length = Some(0) (not only when it is None), ensuring any prior larger minimum (e.g., from Variable) is overridden to 0.

</details>

### 5. `completions.rs:1931` — Typo in test comment
- **Status**: TODO
- **Severity**: Trivial
- **File**: `crates/djls-ide/src/completions.rs`
- **Fix**: Comment says `{{% and {{` but should be `{% and {{`. One-char fix.

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: crates/djls-ide/src/completions.rs | **Lines**: 1931 to 1941 | **Type**: potential_issue
>
> In @crates/djls-ide/src/completions.rs around lines 1931 - 1941, Update the test comment in the function test_tag_context_preferred_over_variable_when_both_present: change the mistaken delimiter text "{{% and {{" to the correct "{% and {{", so the comment correctly reads that {% and {{ are present (fix the extra { in the first delimiter).

</details>

### 6. `opaque.rs:321-328` — Off-by-one in test comment and assertion
- **Status**: TODO
- **Severity**: Low (test imprecision)
- **File**: `crates/djls-semantic/src/opaque.rs`
- **Fix**: Counting positions: `{% verbatim %}` = 0–13, `opaque` = 14–19, `{% endverbatim %}` = 20–36, `after` = starts at **37**. Comment says position 36, assertion checks 36. Position 36 (closing `}` of endverbatim) is also not opaque, so the test passes—but it's testing the tag, not "after" as intended. Fix comment and assertion to use 37.

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: crates/djls-semantic/src/opaque.rs | **Lines**: 321 to 328 | **Type**: potential_issue
>
> In @crates/djls-semantic/src/opaque.rs around lines 321 - 328, The test test_content_after_verbatim_not_opaque has an off-by-one: the "after" text starts at position 37, not 36; update the inline comment and the assertion to check regions.is_opaque(37) instead of 36. Locate the test function test_content_after_verbatim_not_opaque, which builds the source string and calls compute_regions(&db, source) and uses regions.is_opaque(...), and change the comment and the argument to regions.is_opaque to 37 so they match the actual positions.

</details>

## 🟡 ACCEPT WITH CAVEATS — Worth Doing, But Scope the Fix

### 7. `archive.rs:20-24` — SHA256 case-sensitivity in `verify_sha256`
- **Status**: TODO
- **Severity**: Low (defensive fix)
- **File**: `crates/djls-corpus/src/archive.rs`
- **Fix**: `sha256_hex` outputs lowercase via `{:x}`. If the expected hash comes from an external source in uppercase, comparison fails. In practice this depends on where manifests come from. A one-line `.to_ascii_lowercase()` on `expected` is cheap insurance.

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: crates/djls-corpus/src/archive.rs | **Lines**: 20 to 24 | **Type**: potential_issue
>
> In @crates/djls-corpus/src/archive.rs around lines 20 - 24, The verification in verify_sha256 currently compares sha256_hex(data) to expected verbatim, which fails if expected uses uppercase hex; normalize both sides to the same case (e.g., call sha256_hex(data) and expected.to_ascii_lowercase()) before comparing, and update the error message to show the original expected and actual hex strings (or their normalized forms) using the label parameter so the mismatch remains clear; reference the verify_sha256 function and sha256_hex call when making this change.

</details>

### 8. `expressions.rs:48-57` — Tuple/Subscript don't propagate `ctx`
- **Status**: TODO
- **Severity**: Low (theoretical)
- **File**: `crates/djls-extraction/src/dataflow/eval/expressions.rs`
- **Fix**: The `ctx` is used for bounded call inlining. Tuple elements and subscript bases rarely contain function calls in Django template tag code. If this is intentional to limit inlining depth, add a comment documenting the choice. If not, switch to `eval_expr_with_ctx`.

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: crates/djls-extraction/src/dataflow/eval/expressions.rs | **Lines**: 48 to 57 | **Type**: potential_issue
>
> In @crates/djls-extraction/src/dataflow/eval/expressions.rs around lines 48 - 57, The tuple and subscript branches call eval_expr without propagating ctx, so nested Calls won't resolve module-local functions; change the Tuple branch to map elts.iter().map(|e| eval_expr_with_ctx(e, env, ctx)) and the Subscript branch to evaluate value with eval_expr_with_ctx(value, env, ctx) before calling eval_subscript, ensuring ctx is passed to nested expressions (or, if this omission is intentional to limit inlining, add a brief comment in the Expr::Tuple and Expr::Subscript arms referencing eval_expr vs eval_expr_with_ctx to document the design choice).

</details>

### 9. `expressions.rs:191-201` — `split(None, 1)` doesn't validate second arg
- **Status**: TODO
- **Severity**: Low (theoretical)
- **File**: `crates/djls-extraction/src/dataflow/eval/expressions.rs`
- **Fix**: `split(None, 2)` would be incorrectly modeled as a 2-tuple. In Django template tag reality, `split(None, 1)` is the only idiomatic pattern—but adding a check for the integer literal `1` in `args.args[1]` is easy and correct.

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: crates/djls-extraction/src/dataflow/eval/expressions.rs | **Lines**: 191 to 201 | **Type**: potential_issue
>
> In @crates/djls-extraction/src/dataflow/eval/expressions.rs around lines 191 - 201, The current branch assumes split(None, 1) but only checks that args.args[0] is Expr::NoneLiteral and ignores args.args[1], so calls like split(None, 2) will be mis-modeled; update the condition in expressions.rs to also validate that args.args[1] is the integer literal 1 (match the AST variant used for integer literals in this codebase) before returning AbstractValue::Tuple([SplitElement { index: Index::Forward(0) }, Unknown]); if the second arg is present but not the literal 1, return a conservative AbstractValue (e.g., AbstractValue::Unknown or a variable-length tuple) instead of the two-element tuple so only split(None, 1) yields the 2-tuple shape. Ensure you reference args.args, Expr::NoneLiteral, the integer-literal AST variant, AbstractValue::Tuple, and Index::Forward in the change.

</details>

## ❌ IGNORE — Not Applicable or Wrong

### 10. `main.rs:31-34` — `CARGO_MANIFEST_DIR` is compile-time
- **Status**: SKIP
- **Reason**: This is a **developer tool** (`djls-corpus`), not a distributed binary. It's a cargo binary run from the repo during development to sync corpus data. `CARGO_MANIFEST_DIR` is exactly right here—it finds `manifest.toml` relative to the crate's source. Runtime resolution would break the dev workflow.

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: crates/djls-corpus/src/main.rs | **Lines**: 31 to 34 | **Type**: potential_issue
>
> In @crates/djls-corpus/src/main.rs around lines 31 - 34, The code currently uses env!("CARGO_MANIFEST_DIR") (manifest_dir) which is a compile-time path; change manifest defaulting logic so you resolve the manifest at runtime: if cli.manifest is Some use it, otherwise compute a runtime base (e.g. from std::env::current_exe() parent or current working directory) and join "manifest.toml" to that runtime base to produce manifest_path, then check that manifest_path exists and if not return an error asking the user to pass --manifest; update references around manifest_dir, manifest_path and the code that constructs the default to use this runtime resolution and existence check.

</details>

### 11. `dataflow.rs:98-102` — Hardcoded "parser" and "token" parameter names
- **Status**: SKIP
- **Reason**: Django's template tag compilation functions have a **rigid contract**: `def do_tag(parser, token)`. These names aren't arbitrary—they're the canonical Django parameter names. The code also filters `"tag_name"`, another Django convention. Making these configurable adds complexity for a case that doesn't exist in practice.

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: crates/djls-extraction/src/dataflow.rs | **Lines**: 98 to 102 | **Type**: potential_issue
>
> In @crates/djls-extraction/src/dataflow.rs around lines 98 - 102, The code in dataflow.rs filters out parameters by hardcoded names "parser" and "token" when building named_positions; this breaks when the function's actual parameter names differ. Modify the function that builds named_positions to accept the actual parser and token parameter identifiers (or read them from Env) and replace the hardcoded checks (name != "parser" && name != "token") with comparisons against the passed-in parser_param and token_param; then update the caller (the site that invokes this function at the nearby call site currently passing no params) to pass parser_param and token_param (or ensure Env contains them) so the filter uses the real parameter names.

</details>

### 12. `CONSOLIDATION_PLAN.md:122` — Missing MatchArgSpec strategy
- **Status**: SKIP
- **Reason**: This is a planning document noting a known gap that needs a design decision. The plan already calls out that this "needs adaptation." This is tracked work, not a code bug.

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: CONSOLIDATION_PLAN.md | **Line**: 122 | **Type**: potential_issue
>
> In @CONSOLIDATION_PLAN.md at line 122, The plan lacks a concrete strategy for populating MatchArgSpec now that TagSpec.args was removed; fix by choosing and implementing one explicit approach: update TagIndex::from_tag_specs to extract args from the new source (either extracted args or block spec data) and construct MatchArgSpec instances there, restore the CloseValidation variants and their builder match arms so closer-argument validation runs, and remove the unused _opener_bits/_closer_bits prefixes; specifically wire detailed-opus's match-arg creation to call TagIndex::from_tag_specs which should produce MatchArgSpec for the only-affected {% block %} tag (or hardcode logic for {% block %} if you opt for the pragmatic short-term fix), ensuring CloseValidation variants are reintroduced and exercised by the builder match arms.

</details>

### 13. `CONSOLIDATION_PLAN.md:216` — Missing `module_path_from_corpus_file()`
- **Status**: SKIP
- **Reason**: Same as above—the plan is documenting future work. The helper will be created during implementation.

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: CONSOLIDATION_PLAN.md | **Line**: 216 | **Type**: potential_issue
>
> In @CONSOLIDATION_PLAN.md at line 216, Tests and docs reference module_path_from_corpus_file() but it's missing; create a helper named module_path_from_corpus_file(filePath: string): string in the extraction crate (or the shared test-utils used by corpus tests) that derives the module path from a corpus file path and update all extract_rules(source) calls in intent-opus tests to call extract_rules(source, module_path_from_corpus_file(filePath)); if the helper already exists in detailed-opus, add a Phase 1 step documenting its location and export so intent-opus tests can import and reuse the same module_path_from_corpus_file function.

</details>

### 14. `CONSOLIDATION_PLAN.md:356` — Trait contract for `get_or_create_file`
- **Status**: SKIP
- **Reason**: The plan's risk table already explicitly identifies this as a medium-risk item that needs resolution during Phase 3. CodeRabbit is restating what the plan already says.

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: CONSOLIDATION_PLAN.md | **Line**: 356 | **Type**: refactor_suggestion
>
> In @CONSOLIDATION_PLAN.md at line 356, Before starting Phase 3, verify whether the trait object type used in tracked functions supports get_or_create_file: inspect the trait definitions for SemanticDb and WorkspaceDb and confirm which one declares get_or_create_file (or if it exists only on DjangoDatabase); if the method is not on the WorkspaceDb/SemanticDb trait, update the plan to either add the method to the appropriate trait, change tracked function signatures to accept the concrete type that exposes get_or_create_file, or redesign collect_workspace_extraction_results to use the trait's available API instead of calling get_or_create_file directly so implementation work in Phase 3 is based on a validated trait contract.

</details>

### 15. `extraction-dataflow-analyzer.md:645-647` — Call-resolution depth wording
- **Status**: SKIP
- **Reason**: The "contradiction" is that `compile_fn → helper → split_contents` looks like 2 hops, but `split_contents` is a **built-in string method**, not a resolved function call. "One level of call resolution" correctly means one function-to-function hop. The doc could be clearer, but this is a documentation nit in a dev-internal design doc.

<details>
<summary>Original CodeRabbit Review</summary>

> **File**: docs/dev/extraction-dataflow-analyzer.md | **Lines**: 645 to 647 | **Type**: potential_issue
>
> In @docs/dev/extraction-dataflow-analyzer.md around lines 645 - 647, The document contradicts itself about call-resolution depth: one place says "One level of call resolution" with example compile_fn → helper → split_contents, while lines mentioning "bounded inlining with depth 2" imply a different depth; pick and state a single clear definition (e.g., define "depth 1" as analyze compile_fn and its directly-called helpers (compile_fn → helper) or define "depth 2" as compile_fn → helper → sub-helper), then update all occurrences to use that chosen terminology consistently (replace the phrase "One level of call resolution" or "bounded inlining with depth 2" to match) and adjust the example chain (compile_fn, helper, split_contents) to reflect the chosen depth so readers can unambiguously map the numeric depth to the example.

</details>

## Summary

| # | File | Verdict | Priority |
|---|------|---------|----------|
| 1 | `archive.rs` tar entry types | ✅ Accept | **High** |
| 2 | `statements.rs` While body | ✅ Accept | **High** |
| 3 | `types.rs` debug_assert | ✅ Accept | **Medium** |
| 4 | `match_arms.rs` Wildcard min | ✅ Accept | **Medium** |
| 5 | `completions.rs` comment typo | ✅ Accept | Trivial |
| 6 | `opaque.rs` off-by-one | ✅ Accept | Low |
| 7 | `archive.rs` SHA256 case | 🟡 Accept | Low |
| 8 | `expressions.rs` ctx propagation | 🟡 Comment | Low |
| 9 | `expressions.rs` split validation | 🟡 Accept | Low |
| 10 | `main.rs` CARGO_MANIFEST_DIR | ❌ Ignore | — |
| 11 | `dataflow.rs` hardcoded params | ❌ Ignore | — |
| 12–14 | CONSOLIDATION_PLAN.md (×3) | ❌ Ignore | — |
| 15 | docs depth wording | ❌ Ignore | — |

**6 to fix, 3 minor/comment fixes, 6 to ignore.**

# Ruff Reading List

> Files in the Ruff codebase worth reading in full, in suggested order.
> Local clone at `/path/to/ruff/`.

## Start Here: The Narrowing System

This is the closest analog to what djls-python's constraint system
does. It's 1073 lines and self-contained.

**`crates/ty_python_semantic/src/types/narrow.rs`** (1073 lines)

Read for:
- `NarrowingConstraintsBuilder` (line 319) — 4-field immutable context
- `evaluate_expression_node_predicate` (line 374) — match dispatch
  returning `Option<NarrowingConstraints>`
- `evaluate_expr_eq` (line 478) — detailed constraint extraction with
  inline helper functions
- `evaluate_bool_op` (line 1024) — boolean composition via algebraic
  merge functions
- `evaluate_expr_compare` (line 734) — comparison handling
- The Salsa query wrappers at the top (lines 75-95) — how caching works

## Then: The Inference Builder

9333 lines, but you don't need to read it all. Focus on the patterns.

**`crates/ty_python_semantic/src/types/infer/builder.rs`**

Read these sections:
- `TypeInferenceBuilder` struct definition (line ~296) — builder fields
- `infer_expression_impl` (line 4986) — the main dispatch, returning values
- `store_expression_type` (line 5046) — the one place results are stored
- Any one `infer_*_expression` method to see the pattern in action

## Then: The Semantic Model

How Ruff models Python's scoping and binding semantics.

**`crates/ruff_python_semantic/src/binding.rs`** (860 lines)

Read for:
- `BindingKind` enum (line 427) — 20+ variants, exhaustive domain model
- `BindingFlags` bitflags (line 186) — compact property encoding
- `Binding` struct (line 13) — what data each binding carries

**`crates/ruff_python_semantic/src/scope.rs`** (305 lines)

Read for:
- `Scope` struct (line 19) — per-scope binding storage
- `ScopeKind` enum (line 59) — Python scope types
- `Scopes` type (line 180) — `IndexVec`-based arena storage
- `ScopeId` newtype (line 126) — typed index

**`crates/ruff_python_semantic/src/model.rs`** (2739 lines)

Skim for:
- `SemanticModel` struct (line 35) — the main entry point
- `resolve_load` (line ~418) — name resolution algorithm
- How state is tracked via bitflags, not scope chains

## Optional: The Constraint System

Only if you want to see full DNF constraint algebra.

**`crates/ty_python_semantic/src/types/constraints.rs`** (1487 lines)

Read for:
- `Constraints` trait (search for `pub(crate) trait Constraints`)
- `ConstraintSet` / `ConstraintClause` structs
- `union`/`intersect`/`negate` implementations

## Optional: Lint Rule Pattern

If curious about the RAII diagnostic collection pattern.

**`crates/ruff_linter/src/rules/pyflakes/rules/assert_tuple.rs`** (~50 lines)
- The simplest possible rule. Pure function → report diagnostic.

**`crates/ruff_linter/src/checkers/ast/mod.rs`** (search for `DiagnosticGuard`)
- Guard struct, Drop impl, report_diagnostic method.

**`crates/ruff_linter/src/checkers/ast/analyze/statement.rs`**
- Centralized dispatch: one match that routes to all rules per node type.

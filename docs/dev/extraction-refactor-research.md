# Extraction Crate: Refactor Research

> Research notes from code review of PR #394 and study of Ruff/ty codebase
> patterns. This documents problems with the current `djls-extraction` crate
> and patterns from Ruff that point toward better abstractions.

## The Core Problem

The extraction crate works — it correctly analyzes Django template tag
compile functions and produces valid constraint data. But it's hard to
follow. It reads like agent-generated code: procedural, mutation-heavy,
organized for correctness rather than comprehension. It doesn't leverage
what makes Rust worth writing — the type system, ADTs, making impossible
states unrepresentable.

The fundamental issue: **this is procedural code wearing Rust syntax.**
Functions mutate shared state through `&mut` parameters. The type signatures
don't tell you what a function does — you have to read the body. Everything
is a wrapper around a wrapper around three different calls, all with a
shared state object being passed around.

## PR #394 Review Comments (Summary)

These are the concrete complaints from code review, grouped by theme.

### Naming

- **"extraction" as a crate name** — too vague. The crate does filesystem
  scanning, decorator discovery, abstract interpretation, and block
  structure analysis. "Extraction" papers over all of that.
- **"inventory" naming** — don't like `EnvironmentInventory`. Don't like
  "inspector" naming either (the inspector is just the method, not what
  matters).
- **`TemplateTags` has more than tags** — the name lies about the contents.
- **`Env` is too opaque** — in `dataflow/domain.rs`. Standard CS term but
  disconnected from the narrative of the crate.
- **"extraction" for the crate itself** — (`Cargo.toml:2`) not sold on it.

### Organization

- **`blocks.rs` at 1418 lines** — 4 distinct strategies (opaque detection,
  parse-call extraction, control flow classification, next-token loop
  detection) crammed into one file with 47 functions.
- **`eval.rs` is thin facade + 1000 lines of tests** — you open it
  expecting evaluation logic and find tests for other modules. No human
  organizes code this way.
- **`constraints.rs` at 922 lines** — dense pattern matching that could
  be split.
- **`filter.rs` in corpus crate** — hardcoding Django tag files, why?
- **`dataflow/eval.rs` file** — "What in the world is going on with this
  file? Thin except for tests of other modules??"

### Design Patterns

- **`&mut Vec` accumulator pattern everywhere** — `collect_parser_parse_calls`,
  `collect_skip_past_tokens`, `classify_in_body` (mutates TWO vecs).
  Classic agent-generated C-style pattern, not idiomatic Rust.
- **God-context `AnalysisContext`** — 6 fields, mixed read-only context
  and mutable accumulators. Functions that take `&mut AnalysisContext`
  tell you nothing about what they read vs. write.
- **`extract_from_if_inline` pattern** — mutable state passed around
  everywhere. "I see complecting, I see simple made difficult."
- **Feature gate confusion** — gating `ruff_python_parser` but not
  `ruff_python_ast`. Technically justified but reads as oversight.
- **25 re-exports in `lib.rs`** — flat wall of `cfg`-gated `pub use`
  reads like a generated manifest.
- **Hardcoded external API knowledge scattered** — recognition of
  `parser.compile_filter()`, `token_kwargs()`, `parser.skip_past()`
  spread across `expressions.rs`, `statements.rs`, and `blocks.rs`.
  No single place to see "what Django APIs do we understand?"

### Testing

- **Hardcoded real templatetag functions in tests** — prefer using the
  corpus. Reserve hardcoded source for edge cases.
- **Test placement is confusing** — integration tests in `eval.rs`, unit
  tests scattered, snapshot tests in `lib.rs`.

## What Ruff/ty Teaches Us

### Source Files Referenced

All paths relative to the Ruff repo root at
`/home/josh/projects/astral-sh/ruff/`.

#### ty_python_semantic (type checker)
- `crates/ty_python_semantic/src/types/narrow.rs` — type narrowing
- `crates/ty_python_semantic/src/types/infer/builder.rs` — type inference builder
- `crates/ty_python_semantic/src/types/constraints.rs` — constraint system (DNF)
- `crates/ty_python_semantic/src/types.rs` — `Type<'db>` enum (~40 variants)

#### ruff_python_semantic (linter semantic model)
- `crates/ruff_python_semantic/src/model.rs` — `SemanticModel`, name resolution
- `crates/ruff_python_semantic/src/scope.rs` — `Scopes`, `ScopeKind`, `ScopeId`
- `crates/ruff_python_semantic/src/binding.rs` — `Binding`, `BindingKind` (20+ variants), `BindingId`
- `crates/ruff_python_semantic/src/reference.rs` — reference tracking

#### ruff_linter (lint rules)
- `crates/ruff_linter/src/checkers/ast/mod.rs` — `Checker`, `LintContext`, `DiagnosticGuard`
- `crates/ruff_linter/src/checkers/ast/analyze/statement.rs` — centralized rule dispatch
- `crates/ruff_linter/src/rules/pyflakes/rules/assert_tuple.rs` — simple rule example

### Pattern 1: Methods Return Values, Builder Stores Them

Ruff's `TypeInferenceBuilder` uses `&mut self` — but every inference
method **returns** its result. The mutation is for *storing* results in
a map, not for accumulating them through side-channel bags.

```rust
// crates/ty_python_semantic/src/types/infer/builder.rs:4986-5045
fn infer_expression_impl(&mut self, expression: &ast::Expr) -> Type<'db> {
    let ty = match expression {
        ast::Expr::Name(name) => self.infer_name_expression(name),
        ast::Expr::BinOp(binary) => self.infer_binary_expression(binary),
        // ...every arm returns Type<'db>
    };
    self.store_expression_type(expression, ty);  // one place, explicit
    ty  // AND returns it for composition
}
```

**Contrast with djls-extraction**: `eval_condition` doesn't return
anything — it pushes into `&mut Constraints`. The caller has no
visibility into what happened.

### Pattern 2: Narrowing Builder is Minimal Immutable Context

The `NarrowingConstraintsBuilder` — directly analogous to our constraint
extraction — has only immutable context:

```rust
// crates/ty_python_semantic/src/types/narrow.rs:319-324
struct NarrowingConstraintsBuilder<'db, 'ast> {
    db: &'db dyn Db,
    module: &'ast ParsedModuleRef,
    predicate: PredicateNode<'db>,
    is_positive: bool,
}
```

Four fields. All immutable. Every `evaluate_*` method returns
`Option<NarrowingConstraints<'db>>`. Boolean composition is algebraic:

```rust
// narrow.rs — evaluate_bool_op (simplified)
fn evaluate_bool_op(&mut self, ...) -> Option<NarrowingConstraints<'db>> {
    match bool_op.op {
        BoolOp::And => {
            let left = self.evaluate_expression_predicate(left, is_positive)?;
            let right = self.evaluate_expression_predicate(right, is_positive)?;
            merge_constraints_and(left, right)  // algebraic composition
        }
        BoolOp::Or => {
            let left = self.evaluate_expression_predicate(left, is_positive)?;
            let right = self.evaluate_expression_predicate(right, is_positive)?;
            merge_constraints_or(left, right)
        }
    }
}
```

**Contrast with djls-extraction**: `eval_condition` for `and` creates
a temp `Constraints`, pushes into it, then cherry-picks which fields
to copy back. The logic for "and means drop length constraints but keep
keywords" is encoded in imperative code with a comment, not in types.

### Pattern 3: Newtypes and Arena-Indexed Storage

Ruff's semantic model uses `#[newtype_index]` for typed IDs:

```rust
// crates/ruff_python_semantic/src/scope.rs:126-141
#[newtype_index]
pub struct ScopeId;

// crates/ruff_python_semantic/src/binding.rs:322
#[newtype_index]
pub struct BindingId;
```

All entities stored in `IndexVec<IdType, DataType>` — like a normalized
database. Relationships via typed IDs, not string keys. You can't
accidentally pass a `BindingId` where a `ScopeId` is expected.

**Contrast with djls-extraction**: `Env` is `HashMap<String, AbstractValue>`.
Variable names are strings. No type-level distinction between "the parser
parameter" and "a user's local variable."

### Pattern 4: Exhaustive Enums Encode Domain Semantics

`BindingKind` has 20+ variants, each carrying exactly what it needs:

```rust
// crates/ruff_python_semantic/src/binding.rs:427-551
pub enum BindingKind<'a> {
    Annotation,
    Argument,
    Assignment,
    LoopVar,
    Global(Option<BindingId>),
    Nonlocal(BindingId, ScopeId),
    ClassDefinition(ScopeId),
    FunctionDefinition(ScopeId),
    Import(Import<'a>),
    FromImport(FromImport<'a>),
    // ...
}
```

The type documents and enforces: a `ClassDefinition` *must* carry its
`ScopeId`. A `Nonlocal` *must* reference both its target binding and
scope. `is_macro::Is` generates predicate methods automatically.

### Pattern 5: Salsa Queries Replace Mutable Context Threading

Every major analysis operation in ty is a `#[salsa::tracked]` query:

```rust
// crates/ty_python_semantic/src/types/narrow.rs:75-85
#[salsa::tracked(returns(as_ref), ...)]
fn all_narrowing_constraints_for_expression<'db>(
    db: &'db dyn Db,
    expression: Expression<'db>,
) -> Option<NarrowingConstraints<'db>> {
    let module = parsed_module(db, expression.file(db)).load(db);
    NarrowingConstraintsBuilder::new(db, &module, PredicateNode::Expression(expression), true)
        .finish()
}
```

The query takes immutable inputs, returns a value. Salsa handles caching,
invalidation, and incrementality. No manual `HelperCache`, no
`call_depth` tracking, no `caller_name` recursion guards.

### Pattern 6: DiagnosticGuard (RAII Collection)

Ruff's linter collects diagnostics via a guard pattern:

```rust
// crates/ruff_linter/src/checkers/ast/mod.rs:3274-3291
pub(crate) fn report_diagnostic<T: Violation>(
    &self, kind: T, range: TextRange,
) -> DiagnosticGuard<'_, '_> {
    // Returns guard that auto-pushes to RefCell<Vec<Diagnostic>> on Drop
}
```

Rules are pure functions that call `checker.report_diagnostic()`:

```rust
// crates/ruff_linter/src/rules/pyflakes/rules/assert_tuple.rs
pub(crate) fn assert_tuple(checker: &Checker, stmt: &Stmt, test: &Expr) {
    if let Expr::Tuple(tuple) = &test {
        if !tuple.is_empty() {
            checker.report_diagnostic(AssertTuple, stmt.range());
        }
    }
}
```

No `&mut Vec<Diagnostic>` passed around. The guard handles collection.

### Pattern 7: Explicit Match Dispatch, Not Trait Visitors

Both Ruff's linter and ty use explicit `match` on AST node types rather
than trait-based visitor patterns:

```rust
// crates/ty_python_semantic/src/types/infer/builder.rs
fn infer_statement(&mut self, stmt: &ast::Stmt) {
    match stmt {
        ast::Stmt::FunctionDef(func) => self.infer_function_definition_statement(func),
        ast::Stmt::ClassDef(cls) => self.infer_class_definition_statement(cls),
        ast::Stmt::Return(ret) => self.infer_return_statement(ret),
        // ...
    }
}
```

Each arm delegates to a specific method that returns its result. Clean,
greppable, explicit control flow.

## Implications for djls-extraction Refactor

### What Changes

1. **Constraint extraction returns values.** Every `evaluate_*` /
   `extract_*` / `eval_*` function returns its result. Composition
   happens at the call site, visibly.

2. **`AnalysisContext` splits.** Read-only context (module funcs, config)
   stays as a struct. Accumulated results flow back as return values.
   No more `&mut Constraints` threading.

3. **Domain invariants in types.** "An `and` condition drops length
   constraints" becomes a method on a `Constraint` type. "Position 0
   is the tag name" becomes a newtype. The `Env` gets typed IDs or
   at minimum newtypes for its keys.

4. **Structured trace instead of mutable env.** Instead of "set bits to
   SplitResult, later read it back," produce a sequence of observations
   that a separate function interprets.

5. **`blocks.rs` splits into strategy types.** Four strategies become
   four types or four modules, each returning `Option<BlockTagSpec>`,
   tried in sequence.

6. **Crate rename.** "extraction" → something that actually describes
   what the crate does.

### What Might Not Change

- The underlying analysis logic is sound. The abstract interpretation
  approach, offset tracking, pattern recognition — the algorithms are
  right.
- Feature gating between types and parser may still make sense, just
  needs clearer documentation.
- The `types.rs` type definitions (`TagRule`, `BlockTagSpec`, etc.) are
  fine structurally.

### Open Questions

- Is the `Env` (variable → abstract value mapping) fundamentally needed,
  or can the analysis produce a flat trace of observations? Django compile
  functions are small (5-20 statements), so a trace might be cleaner than
  a full abstract interpreter. **Update**: Yes, the Env is needed — the
  interprocedural dataflow approach with bounded inlining stays. What
  changes is how results flow (return values, not mutation).
- Should constraint composition be algebraic (Ruff's approach with
  `merge_constraints_and`/`or`) or can we get away with simpler patterns
  given the limited domain?
- Where does this crate's boundary sit? Should environment scanning be
  a separate crate, or is it fine as a module within a renamed crate?
- **Decided**: The `HelperCache` goes. Bounded inlining will be expressed
  as Salsa tracked functions. This was explicitly requested and the cache
  was explicitly rejected.

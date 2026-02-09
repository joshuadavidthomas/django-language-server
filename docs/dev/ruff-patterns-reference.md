# Ruff/ty Pattern Reference

> Concrete code examples from the Ruff codebase that demonstrate patterns
> applicable to djls-extraction refactoring. All paths relative to
> `/home/josh/projects/astral-sh/ruff/`.
>
> **Note**: Line numbers are approximate — Ruff is actively developed and
> lines shift frequently. Use `grep` or symbol search to find the actual
> locations. The code patterns and architectural insights are what matter,
> not exact line numbers.

## 1. Type Narrowing (Constraint Extraction from Conditionals)

The closest analog to djls-extraction's `dataflow/constraints.rs`.

### Entry Point

```
crates/ty_python_semantic/src/types/narrow.rs:41-67
```

```rust
pub(crate) fn infer_narrowing_constraint<'db>(
    db: &'db dyn Db,
    predicate: Predicate<'db>,
    place: ScopedPlaceId,
) -> Option<Type<'db>> {
    let constraints = match predicate.node {
        PredicateNode::Expression(expression) => {
            if predicate.is_positive {
                all_narrowing_constraints_for_expression(db, expression)
            } else {
                all_negative_narrowing_constraints_for_expression(db, expression)
            }
        }
        PredicateNode::Pattern(pattern) => { /* similar */ }
        PredicateNode::ReturnsNever(_) => return None,
        PredicateNode::StarImportPlaceholder(_) => return None,
    };
    constraints.and_then(|c| c.get(&place).copied())
}
```

**Key insight**: Takes immutable inputs (`Predicate`, `ScopedPlaceId`),
returns `Option<Type>`. No mutation. Positive/negative handled by
separate cached queries, not a mutable flag.

### Builder

```
crates/ty_python_semantic/src/types/narrow.rs:319-324
```

```rust
struct NarrowingConstraintsBuilder<'db, 'ast> {
    db: &'db dyn Db,
    module: &'ast ParsedModuleRef,
    predicate: PredicateNode<'db>,
    is_positive: bool,
}
```

Compare to djls-extraction's `AnalysisContext` (6 fields, 2 mutable).
This builder has 4 fields, all immutable. The `&mut self` on methods
is only for the builder pattern itself — it doesn't accumulate state.

### Expression Dispatch

```
crates/ty_python_semantic/src/types/narrow.rs:374-402
```

```rust
fn evaluate_expression_node_predicate(
    &mut self,
    expression_node: &ast::Expr,
    expression: Expression<'db>,
    is_positive: bool,
) -> Option<NarrowingConstraints<'db>> {
    match expression_node {
        ast::Expr::Name(_) | ast::Expr::Attribute(_) | ast::Expr::Subscript(_) => {
            self.evaluate_simple_expr(expression_node, is_positive)
        }
        ast::Expr::Compare(expr_compare) => {
            self.evaluate_expr_compare(expr_compare, expression, is_positive)
        }
        ast::Expr::Call(expr_call) => {
            self.evaluate_expr_call(expr_call, expression, is_positive)
        }
        ast::Expr::UnaryOp(unary_op) if unary_op.op == ast::UnaryOp::Not => {
            self.evaluate_expression_node_predicate(&unary_op.operand, expression, !is_positive)
        }
        ast::Expr::BoolOp(bool_op) => {
            self.evaluate_bool_op(bool_op, expression, is_positive)
        }
        _ => None,
    }
}
```

**Key insight**: Every arm returns `Option<NarrowingConstraints>`. Negation
is handled by flipping `is_positive`, not by a separate code path.
Unknown expressions return `None` (no constraint), not `Unknown`.

### Equality Narrowing (Detailed Example)

```
crates/ty_python_semantic/src/types/narrow.rs:478-595
```

This is analogous to djls-extraction's `eval_compare` in `constraints.rs`.
The Ruff version:

- Takes `lhs_ty: Type<'db>` and `rhs_ty: Type<'db>` (already evaluated)
- Returns `Option<Type<'db>>` (the narrowed type, or None)
- Contains detailed inline comments explaining edge cases (e.g.,
  `True == 1` and `False == 0`)
- Uses nested helper functions *defined inside the method* for
  locality (`could_compare_equal`, `can_narrow_to_rhs`)

The function is long but self-contained — all context is in the parameters,
all output is in the return value.

### Salsa Caching

```
crates/ty_python_semantic/src/types/narrow.rs:75-95
```

```rust
#[salsa::tracked(
    returns(as_ref),
    cycle_fn=constraints_for_expression_cycle_recover,
    cycle_initial=constraints_for_expression_cycle_initial,
    heap_size=ruff_memory_usage::heap_size,
)]
fn all_narrowing_constraints_for_expression<'db>(
    db: &'db dyn Db,
    expression: Expression<'db>,
) -> Option<NarrowingConstraints<'db>> {
    let module = parsed_module(db, expression.file(db)).load(db);
    NarrowingConstraintsBuilder::new(db, &module, PredicateNode::Expression(expression), true)
        .finish()
}
```

The query IS the cache. No manual `HelperCache`, no `call_depth` tracking.
Cycle recovery handles recursion automatically.

## 2. Type Inference Builder

The main AST-walking pattern in ty.

### Builder Structure

```
crates/ty_python_semantic/src/types/infer/builder.rs:296-358
```

```rust
pub(super) struct TypeInferenceBuilder<'db, 'ast> {
    context: InferContext<'db, 'ast>,
    index: &'db SemanticIndex<'db>,
    region: InferenceRegion<'db>,

    // Results (returned at end, not side-channel bags)
    expressions: FxHashMap<ExpressionNodeKey, Type<'db>>,
    bindings: VecMap<Definition<'db>, Type<'db>>,
    declarations: VecMap<Definition<'db>, TypeAndQualifiers<'db>>,

    // Builder state
    scope: ScopeId<'db>,
    deferred: VecSet<Definition<'db>>,
    return_types_and_ranges: Vec<TypeAndRange<'db>>,
    // ...
}
```

**Key insight**: Results are stored in typed maps (`expressions`,
`bindings`, `declarations`) that are returned as a unit at the end.
They're not scattered across multiple `&mut` parameters passed to
child functions.

### Expression Dispatch

```
crates/ty_python_semantic/src/types/infer/builder.rs:4986-5045
```

```rust
fn infer_expression_impl(&mut self, expression: &ast::Expr) -> Type<'db> {
    let ty = match expression {
        ast::Expr::NoneLiteral(_) => Type::none(self.db()),
        ast::Expr::NumberLiteral(lit) => self.infer_number_literal_expression(lit),
        ast::Expr::BooleanLiteral(lit) => self.infer_boolean_literal_expression(lit),
        ast::Expr::Name(name) => self.infer_name_expression(name),
        ast::Expr::BinOp(binary) => self.infer_binary_expression(binary),
        ast::Expr::Compare(compare) => self.infer_compare_expression(compare),
        ast::Expr::Call(call) => self.infer_call_expression(call),
        // ... ~30 cases, every arm returns Type<'db>
    };

    self.store_expression_type(expression, ty);
    ty
}
```

**Pattern**: `match` → delegate → return → store. The store is explicit
and in one place. The return value is available for the caller to compose.

### Statement Processing

```
crates/ty_python_semantic/src/types/infer/builder.rs (search: fn infer_statement)
```

Same pattern for statements: match on statement kind, delegate to specific
`infer_*_statement` method. Each method processes its subtree and records
results. Control flow (if/else, for, while) handled by the builder's
snapshot/restore/merge pattern for type state.

## 3. Semantic Model (Arena + Typed IDs)

### Scope Storage

```
crates/ruff_python_semantic/src/scope.rs:180-202
```

```rust
pub struct Scopes<'a>(IndexVec<ScopeId, Scope<'a>>);
```

Not a HashMap. An append-only indexed vector. Once pushed, IDs are
permanent. Enables efficient cross-scope lookups via typed indices.

### Binding Storage

```
crates/ruff_python_semantic/src/binding.rs:336
```

```rust
pub struct Bindings<'a>(IndexVec<BindingId, Binding<'a>>);
```

All bindings across all scopes in one global vector. References don't
need scope paths — just `BindingId`.

### Newtype IDs

```
crates/ruff_python_semantic/src/scope.rs:126-141
```

```rust
#[newtype_index]
pub struct ScopeId;

impl ScopeId {
    pub const fn global() -> Self { ScopeId::from_u32(0) }
}
```

Generated via macro: strongly-typed `u32` wrapper, implements `Index`
trait for `IndexVec` access. Zero runtime overhead.

### BindingKind (Exhaustive Domain Enum)

```
crates/ruff_python_semantic/src/binding.rs:427-551
```

20+ variants, each carrying exactly what it needs. `is_macro::Is`
generates predicate methods (`is_import()`, `is_loop_var()`, etc.).

Relevant variants for djls-extraction comparison:
- `Argument` — function parameter
- `Assignment` — variable assignment
- `LoopVar` — for-loop target
- `Import(Import<'a>)` — carries qualified name

Each variant documents its own requirements in the type.

### Bitflags for Properties

```
crates/ruff_python_semantic/src/binding.rs:186-226
```

```rust
bitflags! {
    pub struct BindingFlags: u16 {
        const EXPLICIT_EXPORT = 1 << 0;
        const EXTERNAL = 1 << 1;
        const ALIAS = 1 << 2;
        const NONLOCAL = 1 << 3;
        const GLOBAL = 1 << 4;
        // ...14 flags total
    }
}
```

Compact (2 bytes), fast (bit ops), queryable. Alternative to boolean
fields or enum variants for orthogonal properties.

## 4. Lint Rules (Pure Functions → Structured Output)

### Rule as Violation Struct

```
crates/ruff_linter/src/rules/pyflakes/rules/assert_tuple.rs
```

```rust
#[derive(ViolationMetadata)]
pub(crate) struct AssertTuple;

impl Violation for AssertTuple {
    fn message(&self) -> String {
        "Assert test is a non-empty tuple, which is always `True`".to_string()
    }
}

pub(crate) fn assert_tuple(checker: &Checker, stmt: &Stmt, test: &Expr) {
    if let Expr::Tuple(tuple) = &test {
        if !tuple.is_empty() {
            checker.report_diagnostic(AssertTuple, stmt.range());
        }
    }
}
```

**Pattern**: Rule is a type. Check is a pure function. Output via
`report_diagnostic` with RAII guard. No `&mut Vec` passing.

### DiagnosticGuard (RAII Collection)

```
crates/ruff_linter/src/checkers/ast/mod.rs:3274-3291
```

```rust
pub(crate) fn report_diagnostic<T: Violation>(
    &self, kind: T, range: TextRange,
) -> DiagnosticGuard<'_, '_> {
    DiagnosticGuard {
        context: self,
        diagnostic: Some(kind.into_diagnostic(range, &self.source_file)),
        rule: T::rule(),
    }
}
```

Guard auto-pushes to `RefCell<Vec<Diagnostic>>` on `Drop`.
Can be modified before drop (`.set_fix()`, `.secondary_annotation()`).
Can be defused to prevent emission.

### Centralized Dispatch

```
crates/ruff_linter/src/checkers/ast/analyze/statement.rs:1-1614
```

One function dispatches to all rules for each statement type:

```rust
pub(crate) fn statement(stmt: &Stmt, checker: &mut Checker) {
    match stmt {
        Stmt::Assert(ast::StmtAssert { test, msg, .. }) => {
            if checker.is_rule_enabled(Rule::AssertTuple) {
                pyflakes::rules::assert_tuple(checker, stmt, test);
            }
            // ...more rules
        }
        // ...more statement types
    }
}
```

Centralized, greppable, explicit enable/disable checks.

## 5. Type System (ADT Design)

### Core Type Enum

```
crates/ty_python_semantic/src/types.rs:647-765
```

~40 variants representing the full type lattice. Key patterns:
- Gradual types: `Dynamic(DynamicType)` — Any, Unknown, Todo, Never
- Literal types: `IntLiteral(i64)`, `BooleanLiteral(bool)`
- Compound types: `Union(UnionType<'db>)`, `Intersection(IntersectionType<'db>)`
- Special forms: `AlwaysTruthy`, `AlwaysFalsy`

### Union/Intersection Builders

```
crates/ty_python_semantic/src/types/builder.rs
```

Types constructed via builder that normalizes on construction (flatten
nested unions, remove duplicates, simplify contradictions). Ensures
type invariants at construction time, not at use time.

### Constraint System (DNF)

```
crates/ty_python_semantic/src/types/constraints.rs
```

```rust
pub(crate) trait Constraints<'db>: Clone + Sized {
    fn unsatisfiable(db: &'db dyn Db) -> Self;
    fn always_satisfiable(db: &'db dyn Db) -> Self;
    fn union(&mut self, db: &'db dyn Db, other: Self) -> &Self;
    fn intersect(&mut self, db: &'db dyn Db, other: Self) -> &Self;
    fn negate(self, db: &'db dyn Db) -> Self;
}
```

Algebraic operations on constraints. `ConstraintSet` uses Disjunctive
Normal Form (union of clauses, each clause is intersection of
constraints). Simplification via saturation and subsumption.

## Summary: Patterns to Adopt

| Ruff Pattern | Current djls-extraction | Target |
|---|---|---|
| Methods return values | Functions push into `&mut` bags | Return `Option<Constraint>`, compose at call site |
| Immutable builder context | God-context `AnalysisContext` | Split into read context + returned results |
| Newtype IDs | String keys in HashMap | At minimum, newtypes for positions/indices |
| Exhaustive domain enums | `AbstractValue` + string-keyed `Env` | Richer types encoding invariants |
| Salsa queries for caching | Manual `HelperCache` + `call_depth` | Salsa tracked functions where possible |
| RAII diagnostic collection | `&mut Vec` threading | Guard pattern or return values |
| Explicit match dispatch | Same (this is already fine) | Keep |
| Algebraic constraint composition | Imperative push + selective copy | Trait-based `intersect`/`union`/`negate` |

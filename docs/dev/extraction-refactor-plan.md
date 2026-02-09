# Extraction Crate: Refactor Plan

> Phased plan, ordered by: impact on code clarity, risk, and dependency
> chain. Each phase is independently shippable.
>
> **Important**: The destination is NOT "cleaner procedural code." It's
> type-driven design where domain invariants are encoded in the type system.
> See `extraction-type-driven-vision.md` for the full picture. The phases
> below are a migration path toward that vision, not the vision itself.

## Phase 0: Establish the Test Baseline

Before touching anything, ensure the existing tests are the safety net.

- [ ] Run full test suite, confirm green: `cargo test -p djls-extraction`
- [ ] Run corpus tests: `cargo test -p djls-extraction --test corpus`
- [ ] Run snapshot tests, ensure all accepted: `cargo insta test -p djls-extraction`
- [ ] Run downstream consumers: `cargo test -p djls-semantic -p djls-server`
- [ ] Record current snapshot count and test count as baseline

**This is the ground truth. Every subsequent phase must keep these green.**

## Phase 1: Return Values, Not Mutations

The single highest-impact change. Mechanical, testable, and addresses the
core design complaint.

### 1a: Constraint extraction returns values

**Current:**
```rust
fn eval_condition(expr: &Expr, env: &Env, constraints: &mut Constraints) { ... }
fn extract_from_if_inline(if_stmt: &StmtIf, env: &Env, constraints: &mut Constraints) { ... }
```

**Target:**
```rust
fn eval_condition(expr: &Expr, env: &Env) -> Constraints { ... }
fn extract_from_if_inline(if_stmt: &StmtIf, env: &Env) -> Constraints { ... }
```

With algebraic composition:
```rust
impl Constraints {
    fn merge_or(self, other: Self) -> Self { ... }
    fn merge_and(self, other: Self) -> Self { ... }
}
```

The `and`/`or` semantics (drop length constraints under `and`, keep
keywords) become methods on the type, not imperative code in callers.

**Files touched**: `dataflow/constraints.rs`, `dataflow/eval/statements.rs`

### 1b: `blocks.rs` collection functions return values

**Current:**
```rust
fn collect_parser_parse_calls(body: &[Stmt], parser_var: &str, calls: &mut Vec<ParseCallInfo>)
fn collect_skip_past_tokens(body: &[Stmt], parser_var: &str, tokens: &mut Vec<String>)
fn classify_in_body(body: ..., intermediates: &mut Vec<String>, end_tags: &mut Vec<String>)
```

**Target:**
```rust
fn collect_parser_parse_calls(body: &[Stmt], parser_var: &str) -> Vec<ParseCallInfo>
fn collect_skip_past_tokens(body: &[Stmt], parser_var: &str) -> Vec<String>
fn classify_in_body(body: ...) -> Classification { intermediates: Vec<String>, end_tags: Vec<String> }
```

**Files touched**: `blocks.rs`

### 1c: Expression evaluation returns values (already does, mostly)

`eval_expr` already returns `AbstractValue`. The main issue is
`process_statements` which mutates both `env` and `ctx.constraints`.

Separate the env-mutation (necessary for abstract interpretation) from
constraint accumulation (should be returned):

```rust
fn process_statements(stmts: &[Stmt], env: &mut Env, ctx: &AnalysisContext) -> Constraints
```

`env` mutation stays (it's the interpreter state). Constraints come back
as return values.

**Files touched**: `dataflow/eval/statements.rs`, `dataflow/eval.rs`, `dataflow.rs`

## Phase 2: Split the AnalysisContext

Once constraints flow as return values, the god-context shrinks.

**Current:**
```rust
pub struct AnalysisContext<'a> {
    pub module_funcs: &'a [&'a StmtFunctionDef],  // read-only
    pub caller_name: &'a str,                       // read-only (recursion guard — gone with Salsa)
    pub call_depth: usize,                          // recursion guard (gone with Salsa)
    pub cache: &'a mut HelperCache,                 // gone with Salsa
    pub known_options: Option<KnownOptions>,         // accumulator
    pub constraints: Constraints,                    // accumulator (gone with Phase 1)
}
```

**After Phase 1**, `constraints` is gone (returned). Three more fields
are eliminated by Salsa (Phase 6). What remains:

```rust
/// Immutable context for analysis
pub struct AnalysisContext<'a> {
    pub db: &'a dyn Db,                             // Salsa database
    pub module_funcs: &'a [&'a StmtFunctionDef],    // for helper lookup
}

/// Results accumulated during analysis (returned, not passed in)
pub struct AnalysisResult {
    pub constraints: ConstraintSet,
    pub known_options: Option<OptionLoop>,
}
```

**Files touched**: `dataflow/eval.rs`, `dataflow/eval/statements.rs`,
`dataflow.rs`

## Phase 3: Decompose blocks.rs

Split the 1418-line file into strategy modules. Each strategy is a
function that returns `Option<BlockTagSpec>`:

```
blocks/
    mod.rs          — pub fn extract_block_spec() + shared helpers
    opaque.rs       — parser.skip_past() detection
    parse_calls.rs  — parser.parse(()) extraction + classification
    dynamic_end.rs  — f-string/format end tag patterns
    next_token.rs   — parser.next_token() loop patterns
```

The orchestrator in `mod.rs` tries strategies in order:

```rust
pub fn extract_block_spec(func: &StmtFunctionDef) -> Option<BlockTagSpec> {
    let parser_var = extract_parser_param(func)?;
    opaque::detect(&func.body, &parser_var)
        .or_else(|| parse_calls::detect(&func.body, &parser_var))
        .or_else(|| dynamic_end::detect(&func.body, &parser_var))
        .or_else(|| next_token::detect(&func.body, &parser_var))
}
```

Shared helpers (`is_parser_receiver()`, `extract_string_sequence()`, etc.)
go in `mod.rs` or a `helpers.rs` submodule.

**Files touched**: `blocks.rs` → `blocks/` directory

## Phase 4: Move Environment Scanning Out

Environment scanning is a filesystem crawler, not AST analysis. It
belongs closer to the project/inspector layer.

- [ ] Move `environment/scan.rs` functions to `djls-project`
- [ ] Keep `environment/types.rs` types where consumers can reach them
  (either in `djls-extraction` as today, or in `djls-project` with
  re-exports)
- [ ] The optional `collect_registrations_from_body` call becomes a
  dependency from `djls-project` → `djls-extraction` (which already
  exists)

**Outcome**: `djls-extraction` becomes purely "Python AST → validation
rules." No filesystem operations.

## Phase 5: Rename Things

Do this alongside other changes, not as a standalone phase. Standalone
renames break git blame for no code benefit.

### Crate name
TBD — candidates: `djls-python-analysis`, `djls-tagspec`,
`djls-pyanalyze`, `djls-registration`. Decide after phases 1-4 when
the shape is clearer.

### Type names
- `EnvironmentInventory` → `TemplateLibraries`? `InstalledLibraries`?
- `EnvironmentLibrary` → `TemplateTagLibrary`?
- `EnvironmentSymbol` → `RegisteredSymbol`?
- `ExtractionResult` → `AnalysisResult`? `RegistrationRules`?

### Internal names
- `Env` → `AbstractEnv` or `SymbolTable` or `ValueBindings`
- `HelperCache` → gone entirely (Salsa)

## Phase 6: Replace HelperCache with Salsa

**Non-negotiable.** The `HelperCache` was explicitly rejected. It should
never have been built. Salsa tracked functions replace it.

The `HelperCache` + `call_depth` + `caller_name` in `AnalysisContext` is
a hand-rolled memoization and recursion guard. Salsa does all of this
natively and was explicitly requested for this purpose.

### Current flow (to be eliminated)
```
analyze_compile_function(func, module_funcs, &mut cache)
  → process_statements(body, &mut env, &mut ctx)
    → eval_expr_with_ctx(call_expr, env, Some(ctx))
      → calls::resolve_call(func_name, args, ctx)
        → check cache hit
        → check call_depth >= MAX_CALL_DEPTH
        → check self-recursion via caller_name
        → analyze helper body recursively
        → cache result
```

### Target flow (Salsa)
```rust
#[salsa::tracked(
    cycle_fn = helper_cycle_recover,
    cycle_initial = helper_cycle_initial,
)]
fn analyze_helper(
    db: &dyn Db,
    helper: InternedHelper,   // Salsa interned key
    arg_abstractions: InternedArgs,
) -> AbstractValue {
    // Salsa handles: caching, cycle detection, invalidation
    ...
}
```

This eliminates:
- `HelperCache` struct entirely
- `call_depth` field
- `caller_name` field
- Manual recursion guards
- `dataflow/calls.rs` in its current form

### Technical work required

Salsa requires inputs to be Salsa types. The current helper analysis
takes `&[&StmtFunctionDef]` and `Vec<AbstractValue>` — plain Rust types.
This means `djls-extraction` needs to become Salsa-aware:

- Add `salsa` dependency to the crate
- Design interned types for helper function identity and abstract args
- Use `cycle_fn` / `cycle_initial` for recursion (replaces `call_depth`
  and `caller_name`)

This is real work. See `extraction-type-driven-vision.md` "Known
Complications" for details. The difficulty is not a reason to keep the
hand-rolled cache — it's work that needs to be done.

## Parallel Track: Introduce Domain Types

This runs alongside phases 1-3, not after them. As each phase makes
functions return values, introduce the types that give those values
meaning. See `extraction-type-driven-vision.md` for full type designs.

### T1: SplitPosition (with Phase 1)
Replace bare `i64`/`usize` positions with `SplitPosition` newtype.
`RequiredKeyword.position`, `ChoiceAt.position`, `Index::Forward(usize)`
all become `SplitPosition`. The type encodes that position 0 is the
tag name and provides `arg_index()` for conversion.

**Note**: This is a cross-crate change — `djls-semantic`'s
`rule_evaluation.rs` reads these positions. Plan accordingly.

### T2: TokenSplit (with Phase 1c)
Replace `SplitResult { base_offset, pops_from_end }` and
`SplitLength { base_offset, pops_from_end }` with `TokenSplit`.
Offset arithmetic moves into `resolve_index()` / `resolve_length()`
methods. The scattered `+ base_offset + pops_from_end` calculations
in constraints.rs collapse into method calls.

### T3: Guard (with Phase 1a)
Replace the `body_raises_template_syntax_error() + eval_condition()`
two-step with a `Guard` type. Construction validates the error-raise
pattern. `.constraint()` method returns `ConstraintSet`.

Note: currently used in one call site. Evaluate whether the type adds
enough clarity to justify its existence vs. a better function signature.

### T4: ConstraintSet with algebra (with Phase 1a)
Replace `Constraints { arg_constraints, required_keywords, choice_at }`
with `ConstraintSet` that has `and()`/`or()` methods encoding the
boolean composition semantics. The "drop length under and" rule becomes
a method, not a comment.

### T5: BlockEvidence (with Phase 3)
Replace the monolithic blocks.rs with `BlockEvidence` enum — each
variant is a detected pattern, `interpret()` converts to `BlockTagSpec`.
Observation separated from interpretation.

### T6: CompileFunction (with Phase 2)
Validated input type. Construction from `StmtFunctionDef` guarantees
parser/token params exist. Eliminates `map_or("parser", ...)` fallbacks.

### T7: OptionLoop (with Phase 2)
Replace `ctx.known_options: Option<KnownOptions>` side-channel with
`OptionLoop` returned from pattern detection. First-class type instead
of mutable field on god-context.

## Phase 7: (Future) Full Pattern-Recognition Model

Once types are in place, evaluate whether the analysis can shift from
"abstract interpreter" to "pattern recognizer" — the vision described
in `extraction-type-driven-vision.md` where `analyze_compile_function`
is 10 lines of composition:

```rust
fn analyze_compile_function(func: &CompileFunction) -> TagRule {
    let split = find_token_split(func)?;
    let guards = find_guards(func.body);
    let constraints = guards.iter()
        .map(|g| g.constraint(&split))
        .fold(ConstraintSet::default(), ConstraintSet::or);
    let args = find_argument_names(func.body, &split);
    let options = find_option_loop(func.body, &split);
    TagRule::from_parts(constraints, args, options)
}
```

This is the destination. Everything before it is the path there.

## Known Risks and Complications

See `extraction-type-driven-vision.md` "Known Complications" for full
details. Summary:

### Salsa integration is real work (Phase 6)
Salsa requires interned/tracked inputs. The current helper analysis takes
`&[&StmtFunctionDef]` and `Vec<AbstractValue>` — plain Rust types. Need
to design interned types and integrate Salsa into the crate. This is
non-trivial but non-negotiable.

### Return-value overhead (Phase 1)
Returning `ConstraintSet` instead of `&mut Constraints` means allocating
vectors at each recursive call. Mitigate with `SmallVec`. Profile
before/after.

### SplitPosition is cross-crate (Type Track T1)
Changing `RequiredKeyword.position` from `i64` to `SplitPosition` touches
`djls-semantic`'s rule evaluation. Not just an internal refactor.

### Environment scanning has a coupling (Phase 4)
`scan_environment_with_symbols()` calls `collect_registrations_from_body()`
— that's AST analysis. Moving scanning to `djls-project` means
`djls-project` depends on `djls-extraction` for that function (dependency
already exists, but the coupling is real).

### blocks.rs has shared helpers (Phase 3)
The 4 strategies share `is_parser_receiver()`, `extract_string_sequence()`,
etc. Split needs a shared `helpers.rs` or `mod.rs` — not perfectly clean.

## Execution Notes

- **Each phase keeps tests green.** No phase depends on a future phase.
- **Phase 1 changes the internal pattern** from "mutate shared state" to
  "return values and compose." Public API unchanged.
- **Phases 1-3 touch only `djls-extraction` internals.** The public API
  (`extract_rules()` → `ExtractionResult`) stays the same.
- **Type track T1 (SplitPosition) is cross-crate** — do it when ready
  to update consumers too.
- **Phase 4 touches crate boundaries** — the seam is mostly clean but
  `scan_environment_with_symbols` has a coupling to address.
- **Phase 5 (rename) happens alongside other changes**, not standalone.
- **Phase 6 is non-negotiable.** The HelperCache goes, Salsa replaces it.
  The technical challenges are real but must be solved, not avoided.

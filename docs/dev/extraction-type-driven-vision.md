# Extraction Crate: Type-Driven Vision

> This document describes what the extraction crate should look like when
> it's actually leveraging Rust. Not just "return values instead of mutating"
> — that's plumbing. This is about encoding the domain in the type system
> so the code becomes self-documenting and impossible states are
> unrepresentable.

## The Domain, Stated Simply

Django template tag compile functions follow a **small number of idioms**.
They are not arbitrary Python programs. They are 5-20 line functions that
do some combination of:

1. Split the token contents into parts
2. Mutate the split (slice, pop, reassign)
3. Check how many parts there are
4. Check that specific positions have specific keywords
5. Parse block structure (nested templates)
6. Loop over remaining options
7. Delegate to helper functions

The current abstract interpretation approach is **correct** — you need
dataflow analysis to track mutations like `bits = bits[1:]` and
`bits.pop(0)`, and you need bounded inlining to follow helpers. What's
wrong is not the technique but how it's implemented: mutable state bags
instead of typed returns, string-keyed hash maps instead of domain types,
a hand-rolled cache instead of Salsa.

The type system should encode the domain concepts that the analysis
discovers, not just wrap procedural mutation in Rust syntax.

## The Key Types

### CompileFunction: A Validated Input

Before analysis begins, validate and type the input:

```rust
/// A validated compile function ready for analysis.
///
/// Construction guarantees: has parser and token parameters,
/// has a body to analyze.
struct CompileFunction<'a> {
    parser: ParamName<'a>,
    token: ParamName<'a>,
    body: &'a [Stmt],
}

/// A parameter name. Not just a &str — it's a name we've confirmed
/// exists in the function signature.
#[derive(Debug, Clone, Copy)]
struct ParamName<'a>(&'a str);

impl<'a> CompileFunction<'a> {
    fn from_ast(func: &'a StmtFunctionDef) -> Option<Self> {
        let parser = func.parameters.args.first()?;
        let token = func.parameters.args.get(1)?;
        Some(Self {
            parser: ParamName(parser.parameter.name.as_str()),
            token: ParamName(token.parameter.name.as_str()),
            body: &func.body,
        })
    }
}
```

No more `map_or("parser", ...)` fallbacks scattered through the code.
If you have a `CompileFunction`, you know the params exist.

### SplitPosition: Not Just a usize

Positions in `split_contents()` are not raw numbers. Position 0 is
*always* the tag name. Positions 1+ are arguments. Negative positions
index from the end. This should be in the types:

```rust
/// Position within a `token.split_contents()` result.
///
/// In Django, position 0 is always the tag name. This type makes
/// that invariant explicit and provides safe conversion to argument
/// indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SplitPosition {
    /// Absolute position from start (0 = tag name, 1 = first arg)
    Forward(usize),
    /// Position from end (-1 = last, -2 = second to last)
    Backward(usize),
}

impl SplitPosition {
    fn is_tag_name(&self) -> bool {
        matches!(self, Self::Forward(0))
    }

    /// Convert to 0-based argument index (None if this is the tag name)
    fn arg_index(&self) -> Option<usize> {
        match self {
            Self::Forward(0) => None,
            Self::Forward(n) => Some(n - 1),
            Self::Backward(_) => None,  // can't statically resolve
        }
    }
}
```

Now `RequiredKeyword` doesn't have a bare `position: i64` — it has a
`SplitPosition` that encodes what it means.

### TokenSplit: The Result of split_contents()

The current code tracks `base_offset` and `pops_from_end` as fields on
`SplitResult`. This is tracking *how the split has been mutated*. Make
it a proper type:

```rust
/// Represents the result of `token.split_contents()` as it flows
/// through the compile function, tracking mutations.
///
/// Django compile functions commonly do:
/// - `bits = token.split_contents()` (fresh split)
/// - `bits = bits[1:]` (skip tag name)  
/// - `bits.pop(0)` (consume from front)
/// - `bits.pop()` (consume from back)
///
/// This type tracks those mutations so that subsequent length checks
/// and index accesses can be resolved back to original positions.
#[derive(Debug, Clone, PartialEq)]
struct TokenSplit {
    /// How many elements have been removed from the front
    front_offset: usize,
    /// How many elements have been removed from the back
    back_offset: usize,
}

impl TokenSplit {
    fn fresh() -> Self {
        Self { front_offset: 0, back_offset: 0 }
    }

    fn after_slice_from(&self, start: usize) -> Self {
        Self {
            front_offset: self.front_offset + start,
            back_offset: self.back_offset,
        }
    }

    fn after_pop_front(&self) -> Self {
        Self {
            front_offset: self.front_offset + 1,
            back_offset: self.back_offset,
        }
    }

    fn after_pop_back(&self) -> Self {
        Self {
            front_offset: self.front_offset,
            back_offset: self.back_offset + 1,
        }
    }

    /// Resolve a local index (within the mutated split) to an original
    /// SplitPosition (within the full split_contents result).
    fn resolve_index(&self, local: usize) -> SplitPosition {
        SplitPosition::Forward(local + self.front_offset)
    }

    /// Given a length check on the mutated split, what constraint does
    /// that imply on the original split_contents result?
    fn resolve_length(&self, local_length: usize) -> usize {
        local_length + self.front_offset + self.back_offset
    }
}
```

The offset arithmetic — currently scattered across `constraints.rs` with
`+ base_offset + pops_from_end` sprinkled everywhere — lives in ONE
place with named methods. You can't forget to add the offset because
the type forces you through `resolve_index` / `resolve_length`.

### Guard: A Pattern, Not a Boolean Check

The current code does:
```rust
if body_raises_template_syntax_error(&if_stmt.body) {
    eval_condition(&if_stmt.test, env, constraints);
}
```

Two separate concepts smushed together: "is this an error guard?" and
"what does the guard constrain?" Make it a type:

```rust
/// An error guard: an if-condition whose body raises TemplateSyntaxError.
///
/// This type can only be constructed when the body actually raises,
/// so holding a `Guard` means the condition describes invalid input.
struct Guard<'a> {
    condition: &'a Expr,
}

impl<'a> Guard<'a> {
    /// Try to extract a guard from an if-statement.
    /// Returns None if the body doesn't raise TemplateSyntaxError.
    fn from_if(if_stmt: &'a StmtIf) -> Option<Self> {
        if body_raises_template_syntax_error(&if_stmt.body) {
            Some(Self { condition: &if_stmt.test })
        } else {
            None
        }
    }

    /// What constraint does this guard imply?
    fn constraint(&self, split: &TokenSplit) -> ConstraintSet {
        eval_guard_condition(self.condition, split)
    }
}
```

Now the logic is: find guards, ask each guard what it constrains.
The type guarantees you only evaluate conditions that are actually
error guards.

### ConstraintSet: Algebraic, Not a Bag of Vecs

The current `Constraints` is:
```rust
struct Constraints {
    arg_constraints: Vec<ArgumentCountConstraint>,
    required_keywords: Vec<RequiredKeyword>,
    choice_at_constraints: Vec<ChoiceAt>,
}
```

Three bags. When you `and` two constraint sets, you drop length
constraints but keep keywords — that logic is in `eval_condition`
as imperative code with a comment. Encode it:

```rust
/// A single constraint on tag arguments.
#[derive(Debug, Clone)]
enum Constraint {
    /// `len(bits) != 4` → the tag requires exactly 4 tokens
    Length(ArgumentCountConstraint),
    /// `bits[2] != "as"` → position 2 must be "as"
    Keyword(RequiredKeyword),
    /// `bits[1] not in ("on", "off")` → position 1 must be one of these
    Choice(ChoiceAt),
}

/// A set of constraints with algebraic composition.
///
/// Invariants maintained by construction:
/// - `and` drops length constraints (each alone is insufficient under conjunction)
/// - `or` keeps everything (each disjunct is independent)
/// - `negate` inverts the sense (error condition → validity condition is implicit)
#[derive(Debug, Clone, Default)]
struct ConstraintSet {
    constraints: Vec<Constraint>,
}

impl ConstraintSet {
    fn single(c: Constraint) -> Self {
        Self { constraints: vec![c] }
    }

    /// Conjunction: both guard conditions must be true for the error to fire.
    /// Length constraints are dropped (protective guards under `and`).
    /// Keyword/choice constraints are kept.
    fn and(self, other: Self) -> Self {
        let mut kept = Vec::new();
        for c in self.constraints.into_iter().chain(other.constraints) {
            match &c {
                Constraint::Length(_) => {} // dropped under conjunction
                Constraint::Keyword(_) | Constraint::Choice(_) => kept.push(c),
            }
        }
        Self { constraints: kept }
    }

    /// Disjunction: either condition being true fires the error.
    /// Each is an independent constraint.
    fn or(self, other: Self) -> Self {
        let mut merged = self;
        merged.constraints.extend(other.constraints);
        merged
    }
}
```

Now `eval_guard_condition` becomes:

```rust
fn eval_guard_condition(expr: &Expr, split: &TokenSplit) -> ConstraintSet {
    match expr {
        Expr::BoolOp(ExprBoolOp { op: BoolOp::Or, values, .. }) => {
            values.iter()
                .map(|v| eval_guard_condition(v, split))
                .reduce(ConstraintSet::or)
                .unwrap_or_default()
        }
        Expr::BoolOp(ExprBoolOp { op: BoolOp::And, values, .. }) => {
            values.iter()
                .map(|v| eval_guard_condition(v, split))
                .reduce(ConstraintSet::and)
                .unwrap_or_default()
        }
        Expr::Compare(compare) => eval_comparison(compare, split),
        Expr::UnaryOp(ExprUnaryOp { op: UnaryOp::Not, operand, .. }) => {
            eval_negated_condition(operand, split)
        }
        _ => ConstraintSet::default(), // unknown → no constraints (safe)
    }
}
```

Functional. Returns values. Boolean semantics in the type. No comments
needed to explain why length constraints get dropped under `and` — the
method is called `and` and its implementation is the documentation.

### BlockEvidence: What We Found, Not What We Concluded

The current code finds `parser.parse()` calls and immediately tries to
classify them. Separate observation from interpretation:

```rust
/// Evidence of block structure found in a compile function.
///
/// Each variant represents a specific pattern detected in the AST.
/// The `interpret` method converts evidence into a `BlockTagSpec`.
enum BlockEvidence {
    /// `parser.skip_past("endtag")` — opaque block, no nested parsing
    SkipPast { end_tag: String },

    /// `parser.parse(("elif", "else", "endif"))` — standard block parsing
    /// May have multiple calls (first parse, then parse after intermediate)
    ParseCalls { calls: Vec<ParseCall> },

    /// `parser.parse((f"end{tag_name}",))` — dynamic end tag
    DynamicEnd,

    /// Manual `parser.next_token()` loop (e.g., blocktranslate)
    NextTokenLoop { tokens: Vec<String> },
}

struct ParseCall {
    stop_tokens: Vec<String>,
}

impl BlockEvidence {
    fn interpret(self) -> Option<BlockTagSpec> {
        match self {
            Self::SkipPast { end_tag } => Some(BlockTagSpec {
                end_tag: Some(end_tag),
                intermediates: vec![],
                opaque: true,
            }),
            Self::ParseCalls { calls } => classify_parse_calls(&calls),
            Self::DynamicEnd => Some(BlockTagSpec {
                end_tag: None,
                intermediates: vec![],
                opaque: false,
            }),
            Self::NextTokenLoop { tokens } => {
                classify_next_token_loop(&tokens)
            }
        }
    }
}
```

Finding evidence is one step. Interpreting it is another. Each can be
tested independently. Each is simple.

### OptionLoop: A Recognized Pattern, Not Emergent Behavior

The current code detects option loops via `try_extract_option_loop` deep
inside statement processing. It mutates `ctx.known_options` as a side
effect. Make it a first-class pattern:

```rust
/// A while-loop that parses optional keyword arguments.
///
/// Pattern:
/// ```python
/// while remaining:
///     option = remaining.pop(0)
///     if option == "with": ...
///     elif option == "only": ...
///     else: raise TemplateSyntaxError("unknown option")
/// ```
struct OptionLoop {
    options: Vec<String>,
    rejects_unknown: bool,
    allows_duplicates: bool,
}
```

Detection returns `Option<OptionLoop>` — either we matched the pattern
or we didn't. No mutation, no side effects.

## How Analysis Works in This Model

The analysis is still **interprocedural dataflow analysis with bounded
inlining**. What changes is the representation and flow of results.

The abstract interpreter still walks statements, tracks the `TokenSplit`
through mutations, evaluates expressions against bindings, and inlines
helper function bodies when needed. But:

- The interpreter **returns** constraints instead of pushing into bags
- The `TokenSplit` type **encodes** offset arithmetic instead of scattering
  `+ base_offset + pops_from_end` across call sites
- Helper inlining uses **Salsa queries** instead of a hand-rolled cache
- The top-level function composes typed results visibly

```rust
fn analyze_compile_function(func: &CompileFunction, db: &dyn Db) -> TagRule {
    // 1. Walk the body, tracking the token split through mutations
    //    and collecting constraints from error guards.
    //    This IS the abstract interpreter — but it returns results
    //    instead of mutating shared state.
    let analysis = walk_body(func, db);

    // 2. Extract named arguments from bindings
    let args = extract_arg_names(&analysis.bindings, &analysis.constraints);

    // 3. Assemble
    TagRule::from_parts(
        analysis.constraints,
        args,
        analysis.options,
    )
}

/// Result of walking a compile function body.
///
/// Returned by the abstract interpreter, not accumulated in a
/// mutable god-context.
struct BodyAnalysis {
    /// The token split state after all mutations
    split: TokenSplit,
    /// Constraints extracted from error guards
    constraints: ConstraintSet,
    /// Variable bindings (for argument name extraction)
    bindings: Bindings,
    /// Option loop if detected
    options: Option<OptionLoop>,
}
```

Each step is a function that takes typed input and returns typed output.
The abstract interpreter does its job (walk statements, track state,
inline helpers) but communicates through types, not mutation.

### TokenSplit tracks mutations

The abstract interpreter still tracks `bits = bits[1:]` and `bits.pop(0)`.
But the offset arithmetic lives in `TokenSplit` methods:

```rust
impl TokenSplit {
    fn after_slice_from(&self, start: usize) -> Self {
        Self { front_offset: self.front_offset + start, ..self.clone() }
    }

    fn after_pop_front(&self) -> Self {
        Self { front_offset: self.front_offset + 1, ..self.clone() }
    }

    /// Resolve a local index to an original SplitPosition.
    /// This is where `+ base_offset` lives — ONE place, not scattered.
    fn resolve_index(&self, local: usize) -> SplitPosition {
        SplitPosition::Forward(local + self.front_offset)
    }

    fn resolve_length(&self, local_length: usize) -> usize {
        local_length + self.front_offset + self.back_offset
    }
}
```

The interpreter calls `split.after_pop_front()` when it sees `bits.pop(0)`.
When constraint extraction encounters `len(bits) < 3`, it calls
`split.resolve_length(3)` to get the original length. The arithmetic is
in one place, behind a method, not duplicated across constraint handlers.

### Bounded inlining via Salsa

The interprocedural analysis — following calls into helper functions —
stays. What changes is the caching mechanism:

**Current**: Hand-rolled `HelperCache` + `call_depth` + `caller_name`
recursion guard, threaded through `AnalysisContext`.

**Target**: Salsa tracked query with cycle recovery:

```rust
#[salsa::tracked(
    cycle_fn = helper_analysis_cycle_recover,
    cycle_initial = helper_analysis_cycle_initial,
)]
fn analyze_helper(
    db: &dyn Db,
    helper: HelperFunction,
    arg_abstractions: AbstractArgs,
) -> AbstractValue {
    // Salsa handles: memoization, cycle detection, invalidation
    let func = CompileFunction::from_ast(helper.ast(db))?;
    let result = walk_body(&func, db);
    extract_return_value(&result.bindings)
}
```

Salsa gives us:
- **Caching** for free (replaces `HelperCache`)
- **Cycle detection** for free (replaces `call_depth >= MAX_CALL_DEPTH`
  and `caller_name` self-recursion check)
- **Invalidation** for free (if helper source changes, dependents
  recompute)

This is what was explicitly requested and what the consolidation plan
called for. The `HelperCache` should never have been built.

### The Env stays, but gets better types

The variable-to-value binding map is still needed — the interpreter
needs to know that `bits` is a `TokenSplit` and `tag_name` is a
`SplitElement`. But the `Env` gets stronger types:

```rust
/// Abstract value in the interpreter.
///
/// Same concept as today's AbstractValue, but TokenSplit replaces
/// the SplitResult/SplitLength variants with offset tracking.
enum Value {
    Unknown,
    Token,
    Parser,
    Split(TokenSplit),
    Element(SplitPosition),
    Length(TokenSplit),  // len() of a split, carries offsets for resolution
    Int(i64),
    Str(String),
    Tuple(Vec<Value>),
}
```

The `Env` itself could get newtype keys (distinguish parameter names
from local variables) but that's a nice-to-have, not essential. The
critical change is that `Value::Split` carries a `TokenSplit` instead
of bare `base_offset`/`pops_from_end` fields, and `Value::Element`
carries a `SplitPosition` instead of a raw `Index` enum.

## What This Means for the Plan

The phases in `extraction-refactor-plan.md` are still valid as a
migration path, but the **destination** is different:

- Phase 1 (return values) is a stepping stone, not the goal
- Phase 2 (split context) leads toward `BodyAnalysis` as return type
- Phase 3 (decompose blocks.rs) leads toward `BlockEvidence`
- Phase 6 (Salsa) replaces `HelperCache` with tracked queries for
  bounded inlining — this is non-negotiable, it was explicitly requested
- The real target is: **the abstract interpreter returns typed results
  through domain types that encode invariants, with Salsa handling
  caching and cycle detection for interprocedural analysis**

The plan should have a Phase 1.5 or parallel track:

### Type Introduction Track

Alongside making functions return values, introduce the domain types:

1. **`SplitPosition`** — newtype, replace bare `i64`/`usize` positions
2. **`TokenSplit`** — replace `base_offset`/`pops_from_end` fields
3. **`Guard`** — replace the `body_raises_template_syntax_error` +
   `eval_condition` two-step
4. **`ConstraintSet`** with `and`/`or` — replace `Constraints` bag
5. **`BlockEvidence`** — replace the monolithic blocks.rs
6. **`CompileFunction`** — validated input type
7. **`OptionLoop`** — replace `known_options: Option<KnownOptions>` on
   the context

Each type can be introduced incrementally. You don't have to rewrite
everything at once. Introduce `SplitPosition`, use it in new code, migrate
old code over time.

## Known Complications

Things these proposals gloss over that need real answers.

### Salsa integration is not trivial (but is required)

The `HelperCache` is gone. It was explicitly rejected. Salsa tracked
functions replace it. This is non-negotiable.

The technical challenge: Salsa requires all query inputs to be Salsa
types (interned, tracked, or input). The current helper analysis takes:

- `&[&StmtFunctionDef]` — a slice of borrowed AST nodes (not a Salsa type)
- `Vec<AbstractValue>` — plain Rust values (not a Salsa type)
- `&str` for function name (not interned)

You can't just slap `#[salsa::tracked]` on `resolve_call()`. The work
needed to make this happen:

1. **Make extraction Salsa-aware**: Add `salsa` dependency, intern function
   identities and abstract values. This ripples through the crate but
   it's the right thing to do — it aligns with how `djls-server` already
   uses Salsa for `extract_module_rules()`.
2. **Design the query boundary**: Figure out what the tracked function
   signature looks like. Probably keyed on a Salsa-interned function
   identity (file + function name), not on borrowed AST nodes.
3. **Cycle recovery**: Salsa handles cycles natively via `cycle_fn` /
   `cycle_initial`. This replaces `call_depth` and `caller_name`
   recursion guards.

This is real work. It needs to be designed carefully. But "it's hard"
is not a reason to keep a hand-rolled cache that was explicitly asked
not to exist.

### Return-value overhead

Switching from `&mut Constraints` to returning `ConstraintSet` means
allocating and merging vectors at every function boundary. For recursive
calls (boolean expression trees), this could mean many small allocations.

Mitigations:
- `SmallVec<[Constraint; 4]>` for typical constraint counts (most guards
  produce 1-2 constraints)
- The functions are not hot loops — they run once per compile function
  analysis, not per-file-edit
- Profile before and after to verify

### Shared helpers in blocks.rs

The 4 block strategies share helper functions like `is_parser_receiver()`,
`extract_string_sequence()`. Splitting into submodules means either:
- A shared `helpers.rs` (which is fine)
- Or keeping shared functions in `mod.rs`

Not a blocker, but the split isn't as clean as "4 independent files."

### TokenSplit vs SplitResult: is the rename meaningful?

The proposed `TokenSplit` is structurally identical to the current
`SplitResult { base_offset, pops_from_end }`. The value is:
- Named methods (`resolve_index`, `resolve_length`) instead of inline
  arithmetic
- But it IS a rename + method extraction, not a new abstraction

The real win is consolidating the offset arithmetic into methods.
Whether that justifies a new type name is debatable. Could also just
add methods to the existing `AbstractValue::SplitResult` variant's
handling code.

### Guard type: one call site

The `Guard` type wraps a pattern used in ONE place
(`extract_from_if_inline`). Introducing a whole type for a single call
site risks over-engineering. Consider whether the `Guard` concept adds
enough clarity to justify its existence, or whether the current inline
check is fine with better function signatures.

### Incremental migration of SplitPosition

If `SplitPosition` replaces bare `i64` in `RequiredKeyword.position`,
every consumer that reads that field needs updating. That's not just
extraction — it's `djls-semantic`'s `rule_evaluation.rs` and anywhere
else that interprets positions. This is a cross-crate type change, not
an internal refactor.

## The Litmus Test

When this refactor is done, you should be able to:

1. Read the top-level `analyze_compile_function` and understand the
   entire analysis flow from its ~10-line body
2. Look at any intermediate type (`TokenSplit`, `ConstraintSet`, `Guard`,
   `BodyAnalysis`) and know exactly what it represents
3. Add a new analysis pattern (e.g., Django 6.0 match statements) by
   adding a variant to an enum and a match arm, not by threading new
   fields through a god-context
4. Test each layer independently: "does this source produce this
   `TokenSplit`?" "does this guard produce this `ConstraintSet`?"
   "does this helper inline correctly via Salsa?"
5. Never need a comment that says "intentionally dropped" — the type
   makes it impossible to accidentally include
6. See `SplitPosition` in a function signature and know it's a position
   in `split_contents()`, not a bare integer that might be 0-indexed
   or 1-indexed or an argument index or who knows
7. Follow a helper function call and find a Salsa query — not a
   `HelperCache` with `call_depth` and `caller_name` fields on a
   god-context that was explicitly asked not to exist

That's the arm the surgeon builds.

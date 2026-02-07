# Extraction: Dataflow Analyzer for Template Tag Argument Validation

## Goal

Statically determine what arguments a Django template tag accepts —
without running Django — by analyzing the compile function's Python source.

When a user writes `{% regroup musicians by instrument as grouped %}`, the
language server should know that `regroup` requires exactly 5 arguments,
that position 2 must be `"by"`, and that position 4 must be `"as"`. When
they write `{% element "form" class="btn" %}`, it should know that's valid.
When they write `{% widthratio %}` with no arguments, it should flag it.

All of this information is encoded in the compile function's Python code.
The analyzer's job is to extract it.

## Scope

**In scope**: Validation of the content between `{%` and `%}` for tags
registered with `@register.tag`. This means argument counts, required
keywords at specific positions, known option values, and argument names
for completions/snippets.

**Out of scope**:
- Block structure (matching `{% if %}` with `{% endif %}`) — handled by
  our template parser
- `@register.simple_tag` / `@register.inclusion_tag` — these declare
  their argument spec through the Python function signature. No dataflow
  analysis needed; the existing `extract_parse_bits_rule` reads the
  signature directly.
- `@register.filter` — filter arity is determined from the function
  signature (`def lower(value)` vs `def default(value, arg)`). Already
  handled by `extract_filter_arity`.
- Block spec extraction (`parser.parse(("endfor",))`) — already handled
  by `extract_block_spec` via simple method-call detection. Could be
  integrated later but works well as-is.

## Problem with the current approach

The current extraction uses **pattern matching on code shapes**:

- Look for `bits = token.split_contents()`
- Find `if len(bits) < N: raise TemplateSyntaxError(...)`
- Recognize the comparison operator and extract a constraint

This is fragile because:

1. **It matches syntax, not semantics.** `if len(bits) < 4` and
   `if not (len(bits) >= 4)` mean the same thing but require separate
   patterns.

2. **It can't follow data through transformations.** When allauth's
   `do_element` calls `parse_tag(token, parser)` and then checks
   `len(args) > 1`, the pattern matcher doesn't know `args` came from
   `split_contents()` with the tag name popped and kwargs separated.
   It matches `args` by name via a fallback heuristic and produces a
   false positive.

3. **Every new coding style requires a new pattern.** Django 6.0
   introduced `match token.split_contents():` with structural pattern
   matching. The pattern matcher doesn't handle this at all.

4. **It grows into a pile of special cases.** The current `rules.rs` is
   1600 lines of increasingly specific pattern matching — option loops,
   compound conditions, negated ranges, reversed comparisons, etc.

## The new approach: dataflow analysis

Instead of matching code shapes, we track **what the code does with its
inputs**. Every `@register.tag` compile function receives `(parser, token)`
with known types. We trace those values through the function body and
extract constraints from how they're used.

This is a **domain-specific abstract interpreter** — not a general Python
type checker, but a tiny purpose-built analyzer that understands Django's
template tag compilation protocol.

### Why this is tractable

The abstract domain is extremely small:

- `token` has one interesting method: `split_contents()` → `list[str]`
  (also `token.contents.split()` as a variant)
- The result is a list of strings, first element is the tag name
- Compile functions do a small number of things with this list: check
  its length, access elements by index, slice it, pop from it, iterate
  over it, pass it to helpers
- Constraints are expressed as `if condition: raise TemplateSyntaxError`

The control flow is simple — compile functions are typically 10-50 lines
of straight-line code with a few if-statements. No complex class
hierarchies, no dynamic dispatch, no generators, no async.

The scope is single-module — helper functions (like allauth's `parse_tag`)
are always defined in the same file as the compile function that calls
them.

## Corpus survey: how compile functions use `token`

Every pattern below is drawn from the corpus (Django 4.2–6.0, allauth,
wagtail, compressor, sentry). No fabricated examples.

### Source of the argument list

The argument list always comes from one of two methods on `token`:

| Pattern | Semantics | Example |
|---------|-----------|---------|
| `bits = token.split_contents()` | Split respecting quoted strings, tag name at index 0 | Most tags |
| `args = token.contents.split()` | Plain whitespace split, tag name at index 0 | `autoescape`, `load`, `templatetag` |
| `_, rest = token.contents.split(None, 1)` | Split once, discard tag name | `filter` tag |
| `bits = token.split_contents()[1:]` | Split and immediately discard tag name | `firstof`, `ifchanged` |
| `bits = list(token.split_contents())` | Wrapped in `list()` for mutability | `lorem`, `localize` |
| `tag_name, *bits = token.split_contents()` | Star-unpack, tag name separated | wagtail `image` tag |
| `match token.split_contents():` | Structural pattern matching (Python 3.10+) | Django 6.0 `partialdef`, `partial` |
| `tag_name, args, kwargs = parse_tag(token, parser)` | Delegated to helper function | allauth `element` |

### Operations on the argument list

| Operation | Effect on abstract value | Example |
|-----------|--------------------------|---------|
| `len(bits)` | Produces the length (adjusted for base offset) | Nearly every tag |
| `bits[N]` | Extracts element at position N | `regroup`, `for`, `url` |
| `bits[-N]` | Extracts element from end | `cycle` (`bits[-1]`, `bits[-2]`) |
| `bits[N:]` | Slice — new list with base offset shifted by N | `url` (`bits[2:]`), `with` (`bits[1:]`) |
| `bits[:-N]` | Slice from end | `url` (`bits[:-2]`) |
| `bits.pop(0)` | Removes first element, shifts base offset | allauth `do_slot`, `parse_tag` |
| `bits.pop()` | Removes last element | `lorem` (pops from end multiple times) |
| `for bit in bits:` | Iterates over remaining elements | `url`, `do_with`, wagtail `image` |
| `list(bits)` | Copy for mutability — same abstract value | `lorem`, `localize` |

### Constraint patterns (if/raise)

| Pattern | Constraint | Example |
|---------|------------|---------|
| `if len(bits) < N: raise TSE` | Min(N) | `cycle` (`< 2`), `for` (`< 4`) |
| `if len(bits) > N: raise TSE` | Max(N) | `debug` (`> 1`), `resetcycle` (`> 2`) |
| `if len(bits) != N: raise TSE` | Exact(N) | `regroup` (`!= 6`), `widthratio` |
| `if len(bits) >= N: raise TSE` | Max(N-1) | various |
| `if len(bits) <= N: raise TSE` | Min(N+1) | various |
| `if not (N <= len(bits) <= M): raise TSE` | Min(N), Max(M) | `cache` tag |
| `if len(bits) not in (2, 3, 4): raise TSE` | OneOf([2,3,4]) | compressor `compress` |
| `if bits[N] != "keyword": raise TSE` | RequiredKeyword(N, "keyword") | `regroup` (`bits[2] != "by"`) |
| `if bits[-N] == "keyword": ...` | Keyword at position from end | `cycle` (`bits[-1] != "silent"`) |
| `len(bits) != 3 or bits[1] != "as"` | Exact(3) + RequiredKeyword(1, "as") | compound `or` |
| `len(bits) > 3 and bits[2] != "as"` | RequiredKeyword(2, "as") only (length is a guard) | compound `and` |
| `while remaining: option = remaining.pop(0)` | KnownOptions | `include` (`with`, `only`) |
| `match token.split_contents(): case "tag", name:` | Structural constraint | Django 6.0 `partialdef` |

### The `token.contents.split()` variant

Some tags use `token.contents.split()` instead of `token.split_contents()`.
The difference: `split_contents()` respects quoted strings (treating
`"hello world"` as a single token), while `contents.split()` does plain
whitespace splitting.

Both produce a `list[str]` with the tag name at index 0. For validation
purposes, the semantics are the same — we track both as `SplitResult`.
The difference only matters at runtime when resolving filter expressions.

Tags using `contents.split()`: `autoescape`, `load`, `templatetag`,
`get_available_languages`, `get_current_language`, `get_current_language_bidi`,
`get_admin_log`, `static`.

### Helper function delegation

Only one case in the corpus: allauth's `parse_tag`.

```python
def parse_tag(token, parser):
    bits = token.split_contents()
    tag_name = bits.pop(0)
    args = []
    kwargs = {}
    for bit in bits:
        match = kwarg_re.match(bit)
        if match and match.group(1):
            key, value = match.groups()
            kwargs[key] = FilterExpression(value, parser)
        else:
            args.append(FilterExpression(bit, parser))
    return (tag_name, args, kwargs)
```

The helper takes `token`, calls `split_contents()`, separates positional
args from kwargs, and returns a tuple. The compile function destructures
the return value and applies constraints to the processed components.

The dataflow analyzer handles this by analyzing the helper function with
the caller's abstract values bound to the helper's parameters, then
tracking the return value back through tuple unpacking.

## Abstract domain

```
AbstractValue =
  | Unknown                           -- untracked value
  | Token                             -- the token parameter
  | Parser                            -- the parser parameter
  | SplitResult { base_offset: usize } -- result of split_contents()
  | SplitElement { index: Index }     -- single element from split result
  | SplitLength { base_offset: usize } -- len() of a split result
  | Int(i64)                          -- integer constant
  | Str(String)                       -- string constant
  | Tuple(Vec<AbstractValue>)         -- tuple of values
  | List(Vec<AbstractValue>)          -- list with known elements (rare)

Index =
  | Forward(usize)                    -- bits[N] (from start)
  | Backward(usize)                   -- bits[-N] (from end)
```

### Key design decisions

**`Unknown` is the safe default.** Any value we can't track becomes
`Unknown`. Constraints involving `Unknown` values produce no output —
silence is always better than a false positive. This means we don't need
to handle every Python construct, only the ones that matter for template
tag validation.

**`base_offset` tracks list mutations.** When `bits.pop(0)` removes the
first element, the `SplitResult`'s `base_offset` increments. A subsequent
`len(bits) < 3` means "the original list had fewer than 4 elements" (3 + 1
for the popped tag name). When `bits = bits[2:]` slices the list, the new
value gets `base_offset` increased by 2.

**`SplitElement` carries position, not value.** We don't know the runtime
values of the arguments — they come from the template, not the Python code.
But we know *which position* in the split result a variable refers to,
which lets us interpret `bits[2] != "by"` as "position 2 must be 'by'".

**`Tuple` enables function return tracking.** When a helper returns
`(tag_name, args, kwargs)` and the caller does
`tag_name, args, kwargs = helper(...)`, we destructure the `Tuple` and
bind each element to its variable.

## Environment and analysis

```
Env = HashMap<String, AbstractValue>
```

### Initialization

For a `@register.tag` compile function `def do_tag(parser, token):`:

```
env["parser"] = Parser
env["token"] = Token
```

The parameter names are read from the function signature — they're not
always literally `parser` and `token`, though that's the convention.

### Statement processing

Walk statements top-to-bottom. For each statement type:

**Assignment** (`x = expr`):
- Evaluate `expr` against current env → `AbstractValue`
- Bind to target(s):
  - Simple name: `env["x"] = value`
  - Tuple unpack: `a, b, c = Tuple([v1, v2, v3])` → bind each
  - Star unpack: `a, *rest = SplitResult` → `a = SplitElement(0)`,
    `rest = SplitResult { base_offset: 1 }`
  - Subscript: `bits = bits[2:]` → update env with new offset

**Expression statement** (`expr`):
- Handle side effects: `bits.pop(0)` mutates the env entry for `bits`
- Other expression statements are ignored

**If statement** (`if condition: body`):
- If body contains `raise TemplateSyntaxError(...)`:
  - Evaluate `condition` as a constraint → emit if resolvable
- Recurse into body and elif/else for nested if-statements
- **Important**: the env is not forked for branches. We use a simplified
  flow where mutations in branches affect the shared env. This is
  imprecise but sufficient — compile functions rarely have complex
  branching that would require path sensitivity.

**While statement** (`while condition: body`):
- Check for option-parsing pattern: `while remaining: option = remaining.pop(0)`
- If detected, extract known option values from the if/elif/else chain
  in the loop body
- Otherwise, treated as opaque (body is not analyzed for constraints)

**For statement** (`for x in iterable: body`):
- If `iterable` is a tracked `SplitResult`, the loop variable `x` is
  `SplitElement(Unknown)` — we know it's from the split result but not
  which position
- Scan body for constraints on `x` (e.g., keyword checks)
- Otherwise, treated as opaque

**Match statement** (`match expr: case ...:`) — Django 6.0+:
- If `expr` evaluates to `SplitResult`:
  - Each `case` pattern constrains the argument structure
  - `case "tag", name:` → Exact(2) constraint
  - `case "tag", name, "inline":` → Exact(3) + RequiredKeyword(2, "inline")
  - `case _:` with raise → catch-all error
- This is the newest pattern (Python 3.10+, Django 6.0) and should be
  handled from the start

### Expression evaluation

`eval(expr, env) -> AbstractValue`

| Expression | Result |
|------------|--------|
| Name `x` | `env.get("x")` or `Unknown` |
| `token.split_contents()` | `SplitResult { base_offset: 0 }` |
| `token.contents.split()` | `SplitResult { base_offset: 0 }` |
| `token.contents.split(None, 1)` | `Tuple([SplitElement(0), Unknown])` (special case) |
| `len(x)` where x is `SplitResult` | `SplitLength { base_offset: x.base_offset }` |
| `x[N]` where x is `SplitResult` | `SplitElement { index: Forward(N + x.base_offset) }` |
| `x[-N]` where x is `SplitResult` | `SplitElement { index: Backward(N) }` |
| `x[N:]` where x is `SplitResult` | `SplitResult { base_offset: x.base_offset + N }` |
| `x[:-N]` where x is `SplitResult` | `SplitResult { base_offset: x.base_offset }` (length reduced) |
| `list(x)` where x is `SplitResult` | Same `SplitResult` (just a copy) |
| Integer literal | `Int(value)` |
| String literal | `Str(value)` |
| `f(args...)` where f is module-local | Analyze `f` with args bound → return value |
| Anything else | `Unknown` |

### Side effects

Some operations mutate the environment:

| Statement | Mutation |
|-----------|----------|
| `bits.pop(0)` | `env["bits"].base_offset += 1` |
| `bits.pop()` | No offset change (pops from end, affects length checks) |
| `bits = bits[2:]` | Handled as assignment, new `SplitResult` with adjusted offset |

### Constraint extraction from conditions

When `if condition: raise TemplateSyntaxError(...)`:

`eval_constraint(condition, env) -> Option<Constraint>`

| Condition | Constraint |
|-----------|------------|
| `len(sr) < N` | `Min(N + sr.base_offset)` in split_contents terms |
| `len(sr) > N` | `Max(N + sr.base_offset)` |
| `len(sr) != N` | `Exact(N + sr.base_offset)` |
| `len(sr) <= N` | `Min(N + 1 + sr.base_offset)` |
| `len(sr) >= N` | `Max(N - 1 + sr.base_offset)` |
| `not (A <= len(sr) <= B)` | `Min(A + sr.base_offset)`, `Max(B + sr.base_offset)` |
| `len(sr) not in (...)` | `OneOf([... adjusted by sr.base_offset])` |
| `elem != "kw"` where elem is `SplitElement(i)` | `RequiredKeyword(i, "kw")` |
| `cond1 or cond2` | Extract from both (error when either true) |
| `cond1 and cond2` | Extract keywords only (length is a guard, not prescriptive) |
| Involves `Unknown` | `None` (no constraint emitted) |

The `base_offset` adjustment is the key insight that makes this work.
When allauth's `parse_tag` does `bits.pop(0)` (removing the tag name),
all subsequent length checks are against a list that's 1 shorter. The
analyzer tracks this: `len(args) > 1` where `args` has `base_offset: 1`
means "original split_contents has more than 2 elements" — i.e., more
than 1 real argument. That's correct and doesn't trigger false positives.

Wait — actually the allauth `parse_tag` case is more complex than just
offset adjustment. The helper *also* separates positional args from
kwargs, so `args` is not the full remaining list — it's only the
positional subset. This means we can't simply adjust by offset.

### The helper function problem, honestly

For allauth's `parse_tag`, the return values have these semantics:
- `tag_name`: `SplitElement { index: 0 }` — the tag name
- `args`: a **filtered subset** of the split result (positional args only)
- `kwargs`: another **filtered subset** (keyword args only)

The `args` list is not a contiguous slice of the split result. It's a
filtered projection where some elements were routed to `kwargs` instead.
We cannot express this with `SplitResult { base_offset }` alone.

**Options for handling this:**

1. **Track filtered lists as `Unknown`.** When a loop builds a new list
   by appending conditionally, the result is `Unknown`. Constraints on
   `Unknown` values produce no output. This prevents false positives
   without understanding the filtering semantics. This is the minimum
   viable approach.

2. **Track filtered lists as `DerivedList`.** Add a new abstract value
   that means "derived from split_contents but not a contiguous slice."
   Constraints on `DerivedList` could emit "soft" constraints (warnings
   rather than errors) or be skipped entirely.

3. **Model the filtering precisely.** Track which elements went to `args`
   vs `kwargs` based on the conditional in the loop. This is the most
   accurate but requires understanding `kwarg_re.match(bit)` as a
   classifier, which is getting very domain-specific.

**Recommendation**: Option 1 for now. When a value is built by appending
in a loop with conditionals, it becomes `Unknown`. This handles allauth
correctly (no false positive) without overengineering. If future corpus
evidence shows cases where we need more precision, Option 2 is a natural
extension.

## Intra-module function call resolution

When the compile function calls a helper defined in the same module:

1. Find the callee's `StmtFunctionDef` in the module's function list
2. Create a new env with the callee's parameters bound to the caller's
   argument values
3. Analyze the callee's body with this env
4. Track the return value (the value of the last `return` statement's
   expression, or `Unknown` if multiple returns with different values)
5. In the caller, bind the return value to the assignment target

**Scope limit**: One level of call resolution. We don't follow chains of
helpers calling helpers. The corpus shows only direct delegation
(`compile_fn → helper → split_contents`), never deeper chains.

**Recursion guard**: If a function calls itself, return `Unknown` for the
call result.

## Match statement support (Python 3.10+)

Django 6.0's `partialdef` uses structural pattern matching:

```python
match token.split_contents():
    case "partialdef", partial_name, "inline":
        inline = True
    case "partialdef", partial_name, _:
        raise TemplateSyntaxError(...)
    case "partialdef", partial_name:
        inline = False
    case ["partialdef"]:
        raise TemplateSyntaxError("'partialdef' tag requires a name")
    case _:
        raise TemplateSyntaxError("'partialdef' tag takes at most 2 arguments")
```

Each case arm constrains the split result length and positions:
- `case "partialdef", partial_name, "inline":` → length 3, position 0 = "partialdef", position 2 = "inline"
- `case "partialdef", partial_name:` → length 2
- `case _:` with raise → catch-all error

The valid cases are those that DON'T raise. We extract constraints by
collecting the non-error cases and building a union of their shapes.

This is a new extraction pattern that the current pattern matcher doesn't
handle at all. The dataflow analyzer should support it from the start
since Django 6.0 uses it.

## Output model

The analyzer produces the same output types as today — the downstream
consumers (validation, completions, snippets) don't need to change:

- `TagRule` — argument constraints + required keywords + known options +
  extracted args
- `ArgumentCountConstraint` — Min, Max, Exact, OneOf
- `RequiredKeyword` — keyword at a specific position
- `KnownOptions` — values from option loops
- `ExtractedArg` — argument names and positions for completions

The difference is in how these are derived: systematically from dataflow
rather than ad-hoc from pattern matching.

## What we keep, what we replace

### Keep unchanged
- `registry.rs` — registration discovery (`@register.tag`, etc.)
- `blocks.rs` — block spec extraction from `parser.parse()` calls
- `filters.rs` — filter arity from function signatures
- `types.rs` — output types (may evolve slightly)
- `lib.rs` — pipeline orchestration (calls new analyzer instead of old)

### Replace
- `context.rs` — `detect_split_var` and `token_delegated_to_helper`
  subsumed by general value tracking
- `rules.rs` — all 1600 lines of pattern matching replaced by constraint
  extraction from tracked values

### New
- `dataflow.rs` — module root, public API
- `dataflow/domain.rs` — `AbstractValue`, `Index`, `Env` types
- `dataflow/eval.rs` — expression evaluation, statement processing
- `dataflow/constraints.rs` — constraint extraction from conditions
- `dataflow/calls.rs` — intra-module function call resolution

## Build sequence

Each step is independently testable against the corpus.

### Step 1: Domain types and environment

Define `AbstractValue`, `Index`, `Env`. Write basic `eval_expr` for:
- Variable lookup
- Integer and string literals
- `token.split_contents()` → `SplitResult { base_offset: 0 }`
- `token.contents.split()` → `SplitResult { base_offset: 0 }`

**Test**: Parse Django's `do_for`, verify env maps `bits` to
`SplitResult { base_offset: 0 }` after the assignment.

### Step 2: List operations

Handle subscript and slice operations on `SplitResult`:
- `bits[N]` → `SplitElement { index: Forward(N) }`
- `bits[-N]` → `SplitElement { index: Backward(N) }`
- `bits[N:]` → `SplitResult { base_offset: N }`
- `list(bits)` → same value

**Test**: Parse Django's `firstof` (`bits = token.split_contents()[1:]`),
verify `bits` maps to `SplitResult { base_offset: 1 }`.

### Step 3: Constraint extraction from if/raise

When the if-body raises `TemplateSyntaxError`, evaluate the condition:
- `len(SplitResult) < N` → `Min(N + base_offset)`
- `SplitElement != Str` → `RequiredKeyword`
- `or` / `and` / `not` compound handling

**Test**: Parse Django's `regroup`, extract `Exact(6)` +
`RequiredKeyword(2, "by")` + `RequiredKeyword(4, "as")`.
Parse Django's `do_for`, extract `Min(4)` +
`RequiredKeyword(-2 or -3, "in")`.

### Step 4: Side effects (pop, mutation)

Handle `bits.pop(0)` incrementing `base_offset`, `bits.pop()` for end.

**Test**: Parse allauth's `do_slot` (which does `bits.pop(0)` then checks
bits), verify constraints are correctly offset-adjusted.
Parse Django's `lorem` (multiple pops from end + final `len(bits) != 1`
check).

### Step 5: Intra-module function calls

Analyze helper functions with caller's abstract values bound. Track
return values through tuple unpacking.

**Test**: Parse the full allauth `allauth.py` module. Verify `do_element`
produces NO false positive constraints (the `len(args) > 1` check on
the filtered list should produce no constraint because `args` resolves
to `Unknown`).

### Step 6: While-loop option parsing

Detect the `while remaining: option = remaining.pop(0)` pattern. Extract
known options from the if/elif/else chain.

**Test**: Parse Django's `do_include`, extract
`KnownOptions { values: ["with", "only"], rejects_unknown: true }`.
Parse Django's `do_translate`, extract `KnownOptions { values: ["noop",
"context", "as"], ... }`.

### Step 7: Match statement support

Handle `match token.split_contents(): case ...:` patterns. Collect
non-error cases, derive argument structure.

**Test**: Parse Django 6.0's `partialdef_func`, extract constraints.

### Step 8: Integration and validation

- Wire the new analyzer into `lib.rs`, replacing `context.rs` + `rules.rs`
- Run full corpus extraction and snapshot results
- Run corpus template validation tests
- Zero false positives across all corpus templates
- Compare output quality to old system — should be equal or better for
  every compile function

### Step 9: Extracted argument names

Use the tracked values to produce better `ExtractedArg` data:
- Tuple unpacking gives real argument names (`tag_name, item, _in, iterable = bits`)
- Indexed access gives names from assignment targets (`format_string = bits[1]`)
- Required keywords give literal names at known positions

This improves completions and snippet generation.

## Open questions

### Path sensitivity

The current design uses a single env without forking for branches.
This means:

```python
bits = token.split_contents()
if len(bits) == 4:
    tag, a, b, c = bits
    asvar = None
elif len(bits) == 6:
    tag, a, b, c, as_, asvar = bits
```

After this if/elif, what's in env? The last branch that executed wins.
For constraint extraction this doesn't matter (we extract from the
conditions, not from the branch bodies). For argument name extraction,
we might want to merge results from multiple branches.

**Decision**: Start without path sensitivity. Add it later only if
argument name extraction quality requires it.

### `token_kwargs` calls

Several tags use `token_kwargs(remaining_bits, parser)` to parse keyword
arguments. This is an imported function from `django.template.base`, not
a module-local helper. We can't analyze its body.

However, we know its semantics: it consumes keyword arguments from the
remaining bits list. For the purpose of constraint extraction, we can
treat the call as "consumes some elements from the list" and not extract
constraints from remaining length checks after the call.

**Decision**: Treat `token_kwargs` calls as making the list `Unknown`
after the call point. This is conservative but correct.

### `parser.compile_filter` calls

`parser.compile_filter(bits[1])` is just using a value — it doesn't tell
us anything about constraints. We can ignore these for validation
purposes.

For argument names, `compile_filter(bits[1])` tells us position 1 is a
filter expression (a variable or literal), which is useful for
completions.

**Decision**: Track `compile_filter` calls for argument name extraction
but ignore for constraint extraction.

### Django version differences

The same tag may have different compile function implementations across
Django versions. The `partialdef` tag only exists in Django 6.0+. The
`cycle` tag's implementation changed between versions.

The analyzer should handle all observed patterns. The corpus tests across
Django 4.2/5.1/5.2/6.0 serve as regression tests for version
compatibility.

### Performance

The analyzer runs at extraction time (when Python source files change),
not at template validation time. Extraction results are cached via Salsa.
Performance is not critical but should be reasonable — analyzing a module
with 30 compile functions should take < 100ms.

The analyzer processes each function independently (no interprocedural
analysis beyond one-level helper resolution). This is inherently
parallelizable if needed.

## Relationship to existing code

The dataflow analyzer replaces the "how do we extract rules from compile
functions" layer. Everything above and below stays:

```
                  ┌──────────────────────┐
                  │   registry.rs        │  ← finds registrations (unchanged)
                  │   (decorator scan)   │
                  └──────────┬───────────┘
                             │ registration info
                             ▼
              ┌──────────────────────────────┐
              │   dataflow analyzer (NEW)    │  ← replaces context.rs + rules.rs
              │   - tracks token/parser      │
              │   - extracts constraints     │
              │   - resolves helper calls    │
              └──────────────┬───────────────┘
                             │ TagRule, ExtractedArg
                             ▼
              ┌──────────────────────────────┐
              │   blocks.rs / filters.rs     │  ← unchanged
              │   (block specs, filter arity)│
              └──────────────┬───────────────┘
                             │ ExtractionResult
                             ▼
              ┌──────────────────────────────┐
              │   downstream consumers       │  ← unchanged
              │   (validation, completions,  │
              │    snippets, diagnostics)    │
              └──────────────────────────────┘
```

## Success criteria

1. **Zero false positives** across all corpus templates (Django, allauth,
   wagtail, compressor, sentry)
2. **Equal or better constraint extraction** compared to the pattern
   matcher for every compile function in the corpus
3. **Handles Django 6.0 match statements** that the pattern matcher
   cannot handle at all
4. **Handles allauth parse_tag delegation** without the ad-hoc
   `token_delegated_to_helper` workaround
5. **Simpler to extend** — adding support for a new Python construct
   means adding an eval case, not a new pattern
6. **Maintainable** — the code should be structured around the abstract
   domain and transfer functions, not around specific code shapes

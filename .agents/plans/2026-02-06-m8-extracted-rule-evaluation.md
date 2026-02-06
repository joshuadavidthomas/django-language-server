# M8: Extracted Rule Evaluation — Complete Replacement of Static Argument Validation

## Overview

Build the evaluator that applies `ExtractedRule` conditions to template tag arguments, extract argument structure from the Python AST for completions/snippets, remove the old hand-crafted `args`-based validation path entirely, and **prove the system works via corpus-scale template validation tests**.

This milestone closes the gap identified in the M6 post-mortem: extraction rules are computed but never read. After M8, the old `builtins.rs` args and `validate_argument_order()` are gone — not "kept as fallback", gone.

## Current State Analysis

### The Dual-System Problem

- **OLD path (active):** `TagSpec.args` (hand-crafted `TagArg` specs in `builtins.rs`, ~973 lines) → `validate_args_against_spec()` → `validate_argument_order()`. Works for Django builtins only, hand-maintained.
- **NEW path (dead end):** `TagSpec.extracted_rules` (AST-derived `ExtractedRule` conditions) → stored via `merge_extracted_rules()` → **never read by anything**.
- **Completions/snippets** use `spec.args` for position-aware argument completion (`completions.rs` lines 578-610, `snippets.rs`).

### What Extraction Already Produces (from M5)

From `crates/djls-extraction/tests/snapshots/golden__extract_defaulttags_subset.snap`:

```yaml
# for tag
rules:
  - condition: MaxArgCount { max: 3 }
    message: "'for' statements should have at least four words"
  - condition: LiteralAt { index: 2, value: "in", negated: true }
    message: "'for' statements should use 'for x in y'"

# autoescape tag
rules:
  - condition: ExactArgCount { count: 2, negated: true }
    message: "'autoescape' tag requires exactly one argument."
  - condition: Opaque { description: "unrecognized comparison" }
    message: "'autoescape' argument should be 'on' or 'off'"
```

### What's Missing

1. **Rule evaluator:** Nothing reads `TagSpec.extracted_rules`
2. **Argument structure extraction:** No `ExtractedArg` types — can't power completions from extraction
3. **Corpus template validation tests:** Extraction-level corpus tests exist (no panics, yields results), but template-level validation against extracted rules does NOT exist in Rust. The prototype has `test_corpus_templates.py` and `test_real_templates.py` — these were never ported.

### Key Files (current state)

| File | Role |
|------|------|
| `crates/djls-semantic/src/arguments.rs` | OLD validation: `validate_tag_arguments()` → `validate_args_against_spec()` → `validate_argument_order()` |
| `crates/djls-semantic/src/templatetags/specs.rs` | `TagSpec` with both `args` and `extracted_rules` fields; `merge_extracted_rules()`, `merge_block_spec()` |
| `crates/djls-semantic/src/templatetags/builtins.rs` | 973 lines of hand-crafted tag specs with `args:` fields |
| `crates/djls-extraction/src/types.rs` | `ExtractedRule`, `RuleCondition` variants |
| `crates/djls-extraction/src/filters.rs` | **Working model** of parameter extraction from function signature (filter arity) |
| `crates/djls-semantic/src/filter_arity.rs` | **Working model** of extraction→evaluation pipeline |
| `crates/djls-ide/src/completions.rs` | Uses `spec.args` for positional argument completion (lines 578-610, 660+) |
| `crates/djls-ide/src/snippets.rs` | Uses `spec.args` for snippet generation |
| `crates/djls-extraction/tests/corpus.rs` | Extraction-level corpus tests (no panics, yields) |
| `template_linter/tests/test_corpus_templates.py` | **PROTOTYPE** template-level corpus validation (not ported) |
| `template_linter/tests/test_real_templates.py` | **PROTOTYPE** Django shipped template validation (not ported) |

## Desired End State

After M8:

1. **`ExtractedRule` evaluator** validates template tag arguments using conditions extracted from Python AST
2. **Argument structure** extracted from Python AST powers completions/snippets (replaces hand-crafted `args`)
3. **`validate_argument_order()`** and hand-crafted `args:` in `builtins.rs` are removed
4. **Corpus template validation tests** prove zero false positives against Django admin templates, Wagtail, allauth, crispy-forms, Sentry, NetBox
5. **No fallback** to old system — extracted rules are the sole validation source; tags without extracted rules get no argument validation (conservative)

### Verification

```bash
# Unit tests pass
cargo test -q

# Corpus tests pass (requires `just corpus-sync` first)
cargo test -p djls-extraction corpus -- --nocapture
cargo test -p djls-server corpus -- --nocapture

# Clippy clean
cargo clippy -q --all-targets --all-features --fix -- -D warnings
```

### Key Discoveries

- `filters.rs` already extracts from function parameters (line 20-42 of `crates/djls-extraction/src/filters.rs`) — same pattern works for `simple_tag`/`inclusion_tag` argument extraction
- `filter_arity.rs` is a complete working example of extraction→evaluation→accumulator pipeline — follow this pattern exactly for rule evaluation
- `RegistrationInfo` already carries `decorator_kind` and `function_name` — everything needed to find the function and extract parameters
- The prototype's `test_corpus_templates.py` validates templates per-entry with version-aware Django rules and entry-local third-party extraction — this is the gold standard to port
- Corpus manifest includes Django 4.2/5.1/5.2/6.0, plus Wagtail, allauth, crispy-forms, debug-toolbar, compressor, Sentry, NetBox

## What We're NOT Doing

- **Keeping `args` as a fallback:** The M6 deferral that created the dual-system problem. Explicitly rejected.
- **Keeping hand-crafted `args:` in `builtins.rs`:** Removed. Block structure (end tags, intermediates, module mappings) stays.
- **Variable type checking:** Still out of scope.
- **Cross-template state:** `{% extends %}`/`{% include %}` resolution still deferred.
- **Perfect variable names for manual tags:** Best-effort from AST; falls back to generic names like `arg1`.

## Implementation Approach

Six phases, each building on the previous. Phases 1-2 are pure additions (no existing behavior changes). Phase 3 is the switch-over (validation uses new path). Phase 4 is completions. Phase 5 is cleanup. Phase 6 is the proof.

---

## Phase 1: Argument Structure Extraction in `djls-extraction`

### Overview

Add a new extraction pass that derives argument structure from the Python AST. For `simple_tag`/`inclusion_tag`/`simple_block_tag`, this comes directly from the function signature (same pattern as `extract_filter_arity` in `filters.rs`). For manual `@register.tag`, reconstruct from `ExtractedRule` conditions plus optional AST analysis of tuple unpacking and indexed access.

### Changes Required

#### 1. New types in `djls-extraction`

**File:** `crates/djls-extraction/src/types.rs`

Add `ExtractedArg` and `ExtractedArgKind` types:

```rust
/// Extracted argument specification for a template tag.
///
/// Derived from Python AST — either directly from function parameters
/// (`simple_tag`/`inclusion_tag`) or reconstructed from `ExtractedRule`
/// conditions and AST patterns (manual `@register.tag`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractedArg {
    /// Argument name (from parameter name or reconstructed)
    pub name: String,
    /// Kind of argument
    pub kind: ExtractedArgKind,
    /// Whether this argument is required
    pub required: bool,
}

/// Kind of extracted argument.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExtractedArgKind {
    /// Fixed literal keyword (e.g., "in", "as")
    Literal { value: String },
    /// Choice from specific values (e.g., "on"/"off")
    Choice { values: Vec<String> },
    /// Positional variable/expression
    Variable,
    /// Variable number of positional arguments
    VarArgs,
    /// Keyword arguments (**kwargs)
    KeywordArgs,
}
```

Add `extracted_args` field to `ExtractedTag`:

```rust
pub struct ExtractedTag {
    pub name: String,
    pub decorator_kind: DecoratorKind,
    pub rules: Vec<ExtractedRule>,
    pub block_spec: Option<BlockTagSpec>,
    /// Extracted argument structure for completions/snippets
    pub extracted_args: Vec<ExtractedArg>,
}
```

#### 2. Argument extraction for simple/inclusion/block tags

**File:** `crates/djls-extraction/src/args.rs` (new file)

Extract argument structure from function parameters. This mirrors the pattern in `filters.rs` but produces richer output:

```rust
/// Extract argument structure from a `simple_tag`/`inclusion_tag`/`simple_block_tag` function.
///
/// These decorators use `parse_bits()` internally, which parses arguments
/// based on the function signature. The function parameters directly map
/// to template arguments.
///
/// Handles:
/// - Regular params → positional args (required if no default, optional if default)
/// - `*args` → VarArgs
/// - `**kwargs` → KeywordArgs
/// - `takes_context=True` → skip first param ("context")
pub fn extract_args_from_signature(
    parsed: &ParsedModule,
    registration: &RegistrationInfo,
) -> Vec<ExtractedArg> {
    // Find function def, get parameters
    // Skip 'context' if takes_context=True (check decorator kwargs)
    // Map each parameter to ExtractedArg
}
```

Key implementation details:
- Find `takes_context=True` by inspecting decorator call kwargs (the decorator `Call` node is already parsed in `registry.rs`)
- For `simple_block_tag`: skip the `nodelist` parameter (always last, not a template arg)
- Parameter with default → `required: false`; without default → `required: true`
- `*args` → `ExtractedArgKind::VarArgs`
- `**kwargs` → `ExtractedArgKind::KeywordArgs`

Also add support for the `as varname` feature that `simple_tag`/`inclusion_tag` provide automatically. This is a known Django feature: when a simple tag doesn't explicitly handle `as`, Django's `parse_bits` still allows `... as varname` syntax. Add two optional args: `Literal("as")` + `Variable("varname")`, both with `required: false`. Detect whether `as` is suppressed by checking the decorator for `takes_context` vs explicit handling — for now, always append `as varname` as optional for simple/inclusion tags (matches Django runtime behavior).

#### 3. Argument reconstruction for manual tags

**File:** `crates/djls-extraction/src/args.rs` (same file)

For `@register.tag` functions, reconstruct argument structure from:

1. **`ExtractedRule` conditions** (already available):
   - `LiteralAt{index:N, value:V}` → position N-1 is literal V
   - `ChoiceAt{index:N, choices:Vs}` → position N-1 is choice from Vs
   - `ExactArgCount{count:N}` → exactly N-1 args (N includes tag name)
   - `MinArgCount{min:N}` / `MaxArgCount{max:N}` → bounds on arg count

2. **AST analysis** (new, for variable names):
   - Tuple unpacking: `tag_name, item, _in, iterable = bits` → positions 1-3 get names
   - Indexed access: `format_string = bits[1]` → position 1 gets name "format_string"
   - Fall back to generic names: `arg1`, `arg2`, etc.

```rust
/// Reconstruct argument structure for a manual `@register.tag` function.
///
/// Uses a combination of:
/// 1. ExtractedRule conditions (literal positions, choices, arg count bounds)
/// 2. AST analysis (tuple unpacking, indexed access for variable names)
/// 3. Generic fallback names when AST analysis can't determine names
///
/// The index offset is already accounted for: extraction indices include the
/// tag name (index 0), but extracted args exclude it. `LiteralAt{index:2}`
/// becomes arg position 1 (0-indexed in the result).
pub fn reconstruct_args_from_rules_and_ast(
    parsed: &ParsedModule,
    registration: &RegistrationInfo,
    rules: &[ExtractedRule],
    ctx: &FunctionContext,
) -> Vec<ExtractedArg> {
    // 1. Determine total arg count from rules (max bound or exact)
    // 2. Create slots for each position
    // 3. Fill known positions from LiteralAt/ChoiceAt rules (adjusting index by -1)
    // 4. Try to fill variable names from AST (tuple unpacking, indexed access)
    // 5. Fill remaining with generic names
    // 6. Determine required/optional from rules (MinArgCount, etc.)
}
```

For tuple unpacking detection, look for patterns like:
```python
tag_name, arg1, arg2 = token.split_contents()
# or
_, item, _in, iterable = parts
```

For indexed access, look for:
```python
format_string = bits[1]
arg = args[1]
```

Both patterns are straightforward AST walks over the function body. The `FunctionContext` already identifies the split-contents variable, so we know which variable to track.

#### 4. Wire into extraction orchestration

**File:** `crates/djls-extraction/src/lib.rs`

Update `extract_rules()` to call arg extraction:

```rust
for reg in &registrations.tags {
    let ctx = context::FunctionContext::from_registration(&parsed, reg);
    let rules = rules::extract_tag_rules(&parsed, reg, &ctx)?;
    let block_spec = structural::extract_block_spec(&parsed, reg, &ctx)?;
    let extracted_args = args::extract_args(&parsed, reg, &rules, &ctx);

    tags.push(types::ExtractedTag {
        name: reg.name.clone(),
        decorator_kind: reg.decorator_kind.clone(),
        rules,
        block_spec,
        extracted_args,
    });
}
```

Where `extract_args` dispatches based on `decorator_kind`:
- `SimpleTag` / `InclusionTag` / `SimpleBlockTag` → `extract_args_from_signature`
- `Tag` / `HelperWrapper` / `Custom` → `reconstruct_args_from_rules_and_ast`

#### 5. Update golden tests

**File:** `crates/djls-extraction/tests/golden.rs`

The snapshot will now include `extracted_args` for each tag. Update expected output. Example for `for` tag:

```yaml
- name: for
  decorator_kind: Tag
  rules: [...]
  block_spec: { ... }
  extracted_args:
    - name: target
      kind: Variable
      required: true
    - name: in
      kind: { Literal: { value: "in" } }
      required: true
    - name: iterable
      kind: Variable
      required: true
```

And for `now` (simple_tag):

```yaml
- name: now
  decorator_kind: SimpleTag
  rules: []
  block_spec: ~
  extracted_args:
    - name: format_string
      kind: Variable
      required: true
```

### Success Criteria

#### Automated Verification:
- [ ] `cargo test -p djls-extraction` — all existing tests pass, new arg extraction tests pass
- [ ] Golden snapshot includes `extracted_args` for all fixture tags
- [ ] `simple_tag` `now` extracts `[format_string: Variable, required]` from function signature
- [ ] Manual tag `for` reconstructs `[target: Variable, "in": Literal, iterable: Variable]` from rules + AST
- [ ] `autoescape` reconstructs `[mode: Choice("on","off")]` from ChoiceAt or Opaque + AST analysis
- [ ] Corpus extraction tests still pass: `cargo test -p djls-extraction corpus`
- [ ] `cargo clippy -q --all-targets --all-features -- -D warnings`

**Implementation Note:** After completing this phase and all automated verification passes, pause here for manual confirmation before proceeding to Phase 2.

---

## Phase 2: Build Extracted Rule Evaluator in `djls-semantic`

### Overview

Build the function that evaluates `ExtractedRule` conditions against template tag bits. This is the core validation engine that replaces `validate_argument_order()`. Follow the `filter_arity.rs` pattern: resolve, lookup, evaluate, accumulate.

### Changes Required

#### 1. New evaluation module

**File:** `crates/djls-semantic/src/rule_evaluation.rs` (new file)

```rust
/// Evaluate extracted rules against template tag arguments.
///
/// # Index Offset
///
/// Extraction rules use `split_contents()` indices where the tag name is at
/// index 0. The template parser's `bits` EXCLUDES the tag name. The evaluator
/// adjusts: extraction `index N` → `bits[N-1]`.
///
/// Example: extraction says `LiteralAt{index:2, value:"in"}` for the `for` tag.
/// In bits (which has `["item", "in", "items"]`), "in" is at index 1, not 2.
///
/// # Negation Semantics
///
/// Many conditions have a `negated` flag. The rule represents the ERROR condition:
/// - `ExactArgCount{count:2, negated:true}` → error when `len(bits)+1 != 2`
///   (i.e., the tag should have exactly 2 words including tag name)
/// - `LiteralAt{index:2, value:"in", negated:true}` → error when `bits[1] != "in"`
///
/// # Opaque Rules
///
/// `RuleCondition::Opaque` means extraction couldn't simplify the condition.
/// These are silently skipped — no validation, not treated as errors.
///
/// # Error Messages
///
/// `ExtractedRule.message` contains the original Django error message (e.g.,
/// "'for' statements should have at least four words"). When available, this
/// is used directly in the diagnostic. When absent, a generic message is
/// constructed from the condition.
pub fn evaluate_extracted_rules(
    db: &dyn crate::Db,
    tag_name: &str,
    bits: &[String],
    rules: &[djls_extraction::ExtractedRule],
    span: djls_source::Span,
) {
    use djls_extraction::RuleCondition;
    use salsa::Accumulator;

    // bits length in split_contents terms (includes tag name)
    let split_len = bits.len() + 1;

    for rule in rules {
        let violated = match &rule.condition {
            RuleCondition::ExactArgCount { count, negated } => {
                let matches = split_len == *count;
                if *negated { !matches } else { matches }
            }

            RuleCondition::ArgCountComparison { count, op } => {
                use djls_extraction::ComparisonOp;
                match op {
                    ComparisonOp::Lt => split_len < *count,
                    ComparisonOp::LtEq => split_len <= *count,
                    ComparisonOp::Gt => split_len > *count,
                    ComparisonOp::GtEq => split_len >= *count,
                }
            }

            RuleCondition::MinArgCount { min } => {
                split_len < *min
            }

            RuleCondition::MaxArgCount { max } => {
                split_len <= *max  // Note: "max" here means the THRESHOLD
                // MaxArgCount{max:3} + message "at least four words"
                // means error when split_len <= 3
            }

            RuleCondition::LiteralAt { index, value, negated } => {
                // Adjust for index offset: extraction index N → bits[N-1]
                let bits_index = index.checked_sub(1);
                match bits_index {
                    Some(bi) => {
                        let matches = bits.get(bi).map_or(false, |b| b == value);
                        if *negated { !matches } else { matches }
                    }
                    None => false, // index 0 is tag name, always correct
                }
            }

            RuleCondition::ChoiceAt { index, choices, negated } => {
                let bits_index = index.checked_sub(1);
                match bits_index {
                    Some(bi) => {
                        let matches = bits.get(bi).map_or(false, |b| {
                            choices.iter().any(|c| c == b)
                        });
                        if *negated { !matches } else { matches }
                    }
                    None => false,
                }
            }

            RuleCondition::ContainsLiteral { value, negated } => {
                let contains = bits.iter().any(|b| b == value);
                if *negated { !contains } else { contains }
            }

            RuleCondition::Opaque { .. } => {
                // Can't evaluate — skip silently
                continue;
            }
        };

        if violated {
            let error = match &rule.condition {
                // Map to EXISTING diagnostic codes for parity
                RuleCondition::ExactArgCount { .. }
                | RuleCondition::MinArgCount { .. }
                | RuleCondition::MaxArgCount { .. }
                | RuleCondition::ArgCountComparison { .. } => {
                    // S117: ExtractedRuleViolation — uses Django's own error message
                    crate::ValidationError::ExtractedRuleViolation {
                        tag: tag_name.to_string(),
                        message: rule.message.clone().unwrap_or_else(|| {
                            format!("Tag '{tag_name}' argument count violation")
                        }),
                        span,
                    }
                }

                RuleCondition::LiteralAt { .. }
                | RuleCondition::ChoiceAt { .. }
                | RuleCondition::ContainsLiteral { .. } => {
                    crate::ValidationError::ExtractedRuleViolation {
                        tag: tag_name.to_string(),
                        message: rule.message.clone().unwrap_or_else(|| {
                            format!("Tag '{tag_name}' argument violation")
                        }),
                        span,
                    }
                }

                RuleCondition::Opaque { .. } => unreachable!(),
            };

            crate::ValidationErrorAccumulator(error).accumulate(db);
        }
    }
}
```

**Decision: New `ValidationError` variant vs reuse existing.**

Use a single new variant `ExtractedRuleViolation` (S117) that carries the Django error message directly. This is cleaner than mapping to S104-S107 because:
- The extracted rules carry Django's exact error messages (e.g., "'for' statements should have at least four words")
- These messages are more informative than our generic "too many arguments" / "missing argument"
- S104-S107 remain available for user-config `TagArg`-based validation (if kept for config escape hatch)
- The diagnostic message shown to users is the original Django message — maximum fidelity

#### 2. Add `ExtractedRuleViolation` error variant

**File:** `crates/djls-semantic/src/errors.rs`

```rust
#[error("{message}")]
ExtractedRuleViolation {
    tag: String,
    message: String,
    span: Span,
},
```

#### 3. Add diagnostic code mapping

**File:** `crates/djls-ide/src/diagnostics.rs`

```rust
ValidationError::ExtractedRuleViolation { .. } => "S117",
```

Add span extraction:
```rust
| ValidationError::ExtractedRuleViolation { span, .. } => Some(span.into()),
```

#### 4. Register module

**File:** `crates/djls-semantic/src/lib.rs`

```rust
mod rule_evaluation;
pub use rule_evaluation::evaluate_extracted_rules;
```

#### 5. Unit tests for each `RuleCondition` variant

**File:** `crates/djls-semantic/src/rule_evaluation.rs` (inline `#[cfg(test)] mod tests`)

Tests for:
- `ExactArgCount { count: 2, negated: true }` — error when bits+1 != 2
- `ExactArgCount { count: 2, negated: false }` — error when bits+1 == 2
- `MaxArgCount { max: 3 }` — error when bits+1 <= 3
- `MinArgCount { min: 4 }` — error when bits+1 < 4
- `ArgCountComparison { count: 5, op: Gt }` — error when bits+1 > 5
- `LiteralAt { index: 2, value: "in", negated: true }` — error when bits[1] != "in"
- `LiteralAt { index: 2, value: "in", negated: false }` — error when bits[1] == "in"
- `ChoiceAt { index: 1, choices: ["on","off"], negated: true }` — error when bits[0] not in choices
- `ContainsLiteral { value: "as", negated: true }` — error when "as" not in bits
- `Opaque` — always skipped, no error
- Index offset: extraction index 2 → bits[1]
- Out-of-bounds: `LiteralAt { index: 10 }` on short bits → no crash, no error
- Empty rules → no errors
- Multiple rules: first violation stops? No — evaluate all rules, accumulate all errors

### Success Criteria

#### Automated Verification:
- [ ] `cargo test -p djls-semantic rule_evaluation` — all new tests pass
- [ ] Each `RuleCondition` variant has at least one test
- [ ] Index offset tests pass (extraction index N → bits[N-1])
- [ ] Negation semantics tests pass (both true and false)
- [ ] Opaque rules are silently skipped
- [ ] `cargo clippy -q --all-targets --all-features -- -D warnings`

**Implementation Note:** After completing this phase and all automated verification passes, pause here for manual confirmation before proceeding to Phase 3.

---

## Phase 3: Wire Evaluator into Validation Pipeline

### Overview

Replace the old `args`-based validation with the extracted rule evaluator. When `spec.extracted_rules` is non-empty, use the evaluator. When empty, skip argument validation entirely (NOT fallback to old args). Then remove the old validation code.

### Changes Required

#### 1. Replace validation dispatch in `arguments.rs`

**File:** `crates/djls-semantic/src/arguments.rs`

Replace `validate_tag_arguments`:

```rust
pub fn validate_tag_arguments(db: &dyn Db, tag_name: &str, bits: &[String], span: Span) {
    let tag_specs = db.tag_specs();

    // Try opening tag
    if let Some(spec) = tag_specs.get(tag_name) {
        if !spec.extracted_rules.is_empty() {
            crate::rule_evaluation::evaluate_extracted_rules(
                db, tag_name, bits, &spec.extracted_rules, span,
            );
        }
        // No extracted rules = no argument validation (conservative)
        return;
    }

    // Closer and intermediate tags: no extracted rules expected,
    // no argument validation. (Extraction doesn't produce rules for
    // closers/intermediates — they're structural, not argument-bearing.)
    // Exception: {% endblock name %} — see "Validation Gaps" below.
}
```

#### 2. Remove `validate_args_against_spec` and `validate_argument_order`

**File:** `crates/djls-semantic/src/arguments.rs`

Delete:
- `fn validate_args_against_spec()` (entire function)
- `fn validate_argument_order()` (entire function, ~120 lines)
- `use crate::templatetags::TagArg;`
- `use crate::templatetags::TagArgSliceExt;`

The file shrinks from ~500+ lines to ~60 lines (just `validate_all_tag_arguments` + `validate_tag_arguments` + tests).

#### 3. Strip `args:` from `builtins.rs`

**File:** `crates/djls-semantic/src/templatetags/builtins.rs`

For all 31 tag specs, change `args: B(&[...])` to `args: B(&[])`. This removes ~500 lines of hand-crafted argument definitions while preserving block structure (end tags, intermediates, module mappings).

Example before:
```rust
("for", &TagSpec {
    module: B(DEFAULTTAGS_MOD),
    end_tag: Some(EndTag { ... }),
    intermediate_tags: B(&[IntermediateTag { name: B("empty"), ... }]),
    args: B(&[
        TagArg::var("item", true),
        TagArg::syntax("in", true),
        TagArg::var("items", true),
        TagArg::modifier("reversed", false),
    ]),
    opaque: false,
    extracted_rules: Vec::new(),
}),
```

After:
```rust
("for", &TagSpec {
    module: B(DEFAULTTAGS_MOD),
    end_tag: Some(EndTag { ... }),
    intermediate_tags: B(&[IntermediateTag { name: B("empty"), ... }]),
    args: B(&[]),
    opaque: false,
    extracted_rules: Vec::new(),
}),
```

**Note:** The `args` FIELD stays on `TagSpec` for now — it will be populated from extraction (Phase 4) for completions. Only the hand-crafted VALUES are removed.

#### 4. Remove `EndTag.args` and `IntermediateTag.args`

**File:** `crates/djls-semantic/src/templatetags/specs.rs`

Remove the `args` field from `EndTag` and `IntermediateTag`. Extraction doesn't produce argument specs for closers or intermediates. The only validation gap is `{% endblock content %}` (optional block name) — see "Validation Gaps" below.

Before:
```rust
pub struct EndTag {
    pub name: S,
    pub required: bool,
    pub args: L<TagArg>,
}
```

After:
```rust
pub struct EndTag {
    pub name: S,
    pub required: bool,
}
```

Same for `IntermediateTag` — remove `args` field.

Update all constructors, `merge_block_spec`, `From` impls, and test code that creates `EndTag`/`IntermediateTag` with args.

#### 5. Simplify `merge_block_spec`

**File:** `crates/djls-semantic/src/templatetags/specs.rs`

Without `EndTag.args` or `IntermediateTag.args` to protect, the merge becomes simpler. Extraction is the sole source for block structure when the static spec has no end_tag/intermediates. When the static spec already has end_tag/intermediates (from builtins.rs), preserve them (extraction confirms but doesn't override).

```rust
pub fn merge_block_spec(&mut self, block: &djls_extraction::BlockTagSpec) {
    self.opaque = block.opaque;

    if self.end_tag.is_none() {
        if let Some(ref end) = block.end_tag {
            self.end_tag = Some(EndTag {
                name: end.clone().into(),
                required: true,
            });
        }
    }

    if self.intermediate_tags.is_empty() && !block.intermediate_tags.is_empty() {
        self.intermediate_tags = block
            .intermediate_tags
            .iter()
            .map(|it| IntermediateTag {
                name: it.name.clone().into(),
            })
            .collect::<Vec<_>>()
            .into();
    }
}
```

#### 6. Update existing tests

**File:** `crates/djls-semantic/src/arguments.rs` (tests module)

Tests that construct `TagArg` specs and check against the old validation path need updating:
- Tests that verify extracted-rule-based behavior: rewrite to use extracted rules
- Tests that verify specific error types: update to expect `ExtractedRuleViolation`
- The `validate_template` helper still works — it parses templates and runs `validate_nodelist`
- Key regression test: `{% for item in items football %}` must still error

Specific test updates:
- `test_for_rejects_extra_token_after_iterable`: Now caught by extracted rules (MaxArgCount), not args-based validation. Filter for `ExtractedRuleViolation` instead of `TooManyArguments`.
- `test_endblock_with_name_is_valid`: Still valid — no argument validation on closers.
- `test_if_tag_with_comparison_operator`: No argument validation (if tag has no extracted arg-count rules, only expression validation via S114). Should pass cleanly.
- Tests using `check_validation_errors` with custom `TagArg` specs: Remove or convert to use extracted rules.

### Validation Gaps

**`{% endblock content %}`:** The old system validated that `endblock` accepts an optional name via `EndTag.args`. With `EndTag.args` removed, `{% endblock content %}` will no longer be validated (it will silently pass, which is actually correct — Django accepts it). The inverse (`{% endblock %}` when a name is required) was never enforced. **No action needed.**

**Intermediate tag arguments:** `{% elif condition %}` was validated via `IntermediateTag.args`. With that removed, `elif` arguments aren't validated. However, `elif` expression validation is handled by M6's if-expression Pratt parser (S114). **No action needed.**

### Success Criteria

#### Automated Verification:
- [ ] `cargo test -q` — all tests pass (updated tests)
- [ ] `{% for item in items football %}` produces `ExtractedRuleViolation` error
- [ ] `{% for item in items %}` produces no errors
- [ ] `{% for item in items reversed %}` produces no errors (not caught by extraction rules — only `MaxArgCount{max:3}` and `LiteralAt{index:2}` exist)
- [ ] `{% autoescape on %}` produces no errors
- [ ] `{% autoescape %}` produces `ExtractedRuleViolation` (ExactArgCount)
- [ ] `{% endblock content %}` produces no errors
- [ ] `{% csrf_token extra %}` — depends on whether extraction produces rules for csrf_token
- [ ] `{% if and x %}` still produces S114 (expression validation, not argument validation)
- [ ] `cargo clippy -q --all-targets --all-features -- -D warnings`

#### Manual Verification:
- [ ] Open a Django project in editor, verify diagnostics appear correctly
- [ ] Verify diagnostic messages show Django's original error text (from `ExtractedRule.message`)

**Implementation Note:** After completing this phase and all automated verification passes, pause here for manual confirmation before proceeding to Phase 4.

---

## Phase 4: Wire Extracted Args into Completions/Snippets

### Overview

Populate `TagSpec.args` from extraction-derived argument structure so completions and snippets continue working. The `ExtractedArg` → `TagArg` conversion happens during `compute_tag_specs`. The completions and snippets code in `djls-ide` remains unchanged — it reads from `spec.args` as before, but the source is now extraction instead of hand-crafted values.

### Changes Required

#### 1. Add `ExtractedArg` → `TagArg` conversion

**File:** `crates/djls-semantic/src/templatetags/specs.rs`

```rust
impl TagSpec {
    /// Populate `args` from extracted argument structure.
    ///
    /// Called during `compute_tag_specs` to provide completions/snippets data
    /// derived from Python AST instead of hand-crafted specifications.
    pub fn populate_args_from_extraction(
        &mut self,
        extracted_args: &[djls_extraction::ExtractedArg],
    ) {
        if extracted_args.is_empty() {
            return;
        }
        // Only populate if args is currently empty (don't override user config)
        if !self.args.is_empty() {
            return;
        }
        self.args = extracted_args
            .iter()
            .map(|ea| ea.to_tag_arg())
            .collect::<Vec<_>>()
            .into();
    }
}
```

Conversion logic (on `ExtractedArg` or as a free function):

```rust
impl djls_extraction::ExtractedArg {
    fn to_tag_arg(&self) -> TagArg {
        match &self.kind {
            ExtractedArgKind::Literal { value } => TagArg::Literal {
                lit: value.clone().into(),
                required: self.required,
                kind: LiteralKind::Syntax, // Literals from extraction are syntactic
            },
            ExtractedArgKind::Choice { values } => TagArg::Choice {
                name: self.name.clone().into(),
                required: self.required,
                choices: values.iter()
                    .map(|v| Cow::Owned(v.clone()))
                    .collect::<Vec<_>>()
                    .into(),
            },
            ExtractedArgKind::Variable => TagArg::Variable {
                name: self.name.clone().into(),
                required: self.required,
                count: TokenCount::Exact(1),
            },
            ExtractedArgKind::VarArgs => TagArg::VarArgs {
                name: self.name.clone().into(),
                required: self.required,
            },
            ExtractedArgKind::KeywordArgs => TagArg::Assignment {
                name: self.name.clone().into(),
                required: self.required,
                count: TokenCount::Greedy,
            },
        }
    }
}
```

**Note:** This conversion should live in `djls-semantic` (not `djls-extraction`) since `TagArg` is a semantic type. Use a `From` impl or method on a wrapper.

#### 2. Call `populate_args_from_extraction` in `merge_extraction_into_specs`

**File:** `crates/djls-server/src/db.rs`

```rust
fn merge_extraction_into_specs(
    specs: &mut TagSpecs,
    module_path: &str,
    extraction: &ExtractionResult,
) {
    for tag in &extraction.tags {
        if let Some(spec) = specs.get_mut(&tag.name) {
            spec.merge_extracted_rules(&tag.rules);
            if let Some(ref block_spec) = tag.block_spec {
                spec.merge_block_spec(block_spec);
            }
            spec.populate_args_from_extraction(&tag.extracted_args);
        } else {
            let mut new_spec = TagSpec::from_extraction(module_path, tag);
            new_spec.populate_args_from_extraction(&tag.extracted_args);
            specs.insert(tag.name.clone(), new_spec);
        }
    }
}
```

#### 3. Verify completions/snippets work

No changes to `crates/djls-ide/src/completions.rs` or `crates/djls-ide/src/snippets.rs` — they read `spec.args` which is now populated from extraction.

Verify snippet output quality:
- `{% for %}` → `for ${1:target} in ${2:iterable}` (names from AST extraction)
- `{% autoescape %}` → `autoescape ${1|on,off|}` (from ChoiceAt or AST)
- `{% now %}` → `now "${1:format_string}"` (from simple_tag parameter)
- `{% block %}` → special-cased in snippets.rs, unchanged

### Success Criteria

#### Automated Verification:
- [ ] `cargo test -p djls-ide` — all completion/snippet tests pass
- [ ] Snippet test for `for` tag produces meaningful output (not empty)
- [ ] Snippet test for `autoescape` produces choice snippet
- [ ] Snippet test for `block` tag is unchanged (special-cased)
- [ ] Tags without extracted args produce tag-name-only completions (graceful degradation)
- [ ] `cargo clippy -q --all-targets --all-features -- -D warnings`

#### Manual Verification:
- [ ] Open editor, type `{% f` → `for` completion with snippet `for ${1:target} in ${2:iterable} %}...{% endfor %}`
- [ ] Type `{% auto` → `autoescape` completion with choice `${1|on,off|} %}...{% endautoescape %}`
- [ ] Type `{% now` → `now` completion with `"${1:format_string}"`

**Implementation Note:** After completing this phase and all automated verification passes, pause here for manual confirmation before proceeding to Phase 5.

---

## Phase 5: Clean Up Dead Code

### Overview

Remove types and code paths that are now unused after Phases 1-4. The old `args`-based validation is gone (Phase 3), `builtins.rs` args are empty (Phase 3), and `TagSpec.args` is populated from extraction (Phase 4). Clean up the remnants.

### Changes Required

#### 1. Remove unused `TagArg` validation helpers

**File:** `crates/djls-semantic/src/templatetags/specs.rs`

- Remove `TagArgSliceExt` trait and its `find_next_literal()` method (only used by `validate_argument_order`, which is gone)
- Keep `TagArg` enum itself — still used by completions/snippets and user config conversion

#### 2. Simplify `TagSpec` field documentation

Update doc comments on `TagSpec.args` to reflect its new role:

```rust
pub struct TagSpec {
    pub module: S,
    pub end_tag: Option<EndTag>,
    pub intermediate_tags: L<IntermediateTag>,
    /// Argument structure for completions/snippets.
    /// Populated from extraction (Phase 4) or user config.
    /// NOT used for validation (see `extracted_rules`).
    pub args: L<TagArg>,
    pub opaque: bool,
    /// Validation rules from Python AST extraction.
    /// Evaluated by `rule_evaluation::evaluate_extracted_rules`.
    pub extracted_rules: Vec<djls_extraction::ExtractedRule>,
}
```

#### 3. Clean up `from_extraction` constructor

**File:** `crates/djls-semantic/src/templatetags/specs.rs`

Update `TagSpec::from_extraction` to also handle `extracted_args`:

```rust
pub fn from_extraction(module_path: &str, tag: &djls_extraction::ExtractedTag) -> Self {
    let mut spec = TagSpec {
        module: module_path.to_string().into(),
        end_tag: None,
        intermediate_tags: B(&[]),
        args: B(&[]),
        opaque: false,
        extracted_rules: Vec::new(),
    };

    spec.merge_extracted_rules(&tag.rules);
    if let Some(ref block_spec) = tag.block_spec {
        spec.merge_block_spec(block_spec);
    }
    spec.populate_args_from_extraction(&tag.extracted_args);

    spec
}
```

#### 4. Remove old validation error variants that are now unused

Review whether `MissingRequiredArguments`, `TooManyArguments`, `MissingArgument`, `InvalidLiteralArgument`, `InvalidArgumentChoice` are still reachable. If nothing produces them after Phase 3:
- **Keep them** — user config `TagArg` validation (via `from_config_def`) may still produce them if users define `args` in `djls.toml`. These become the user-config-only validation path.
- If we decide to remove user-config `args` support: remove these variants and their S104-S107 codes.
- **Decision for this milestone:** Keep the variants and codes. User config `args` is an escape hatch. The evaluator handles extraction; user config handles edge cases. Simplifying/removing user config `args` is a separate conversation (noted in roadmap open decisions).

Wait — but Phase 3 removed `validate_args_against_spec` and `validate_argument_order`. If user config defines `args`, nothing validates them anymore. We need to decide:

**Decision:** In `validate_tag_arguments` (Phase 3), after checking extracted rules, also check `spec.args` for user-config-provided arguments. If `spec.args` is non-empty AND `spec.extracted_rules` is empty, call the old validation path. This preserves user config as an escape hatch.

**Revised `validate_tag_arguments`:**

```rust
pub fn validate_tag_arguments(db: &dyn Db, tag_name: &str, bits: &[String], span: Span) {
    let tag_specs = db.tag_specs();

    if let Some(spec) = tag_specs.get(tag_name) {
        if !spec.extracted_rules.is_empty() {
            // Primary path: extracted rules from Python AST
            crate::rule_evaluation::evaluate_extracted_rules(
                db, tag_name, bits, &spec.extracted_rules, span,
            );
        } else if !spec.args.is_empty() {
            // Fallback for user-config-defined args only
            // (builtins.rs args are all empty after Phase 3)
            validate_args_against_spec(db, tag_name, bits, span, spec.args.as_ref());
        }
        // Both empty = no argument validation (conservative)
        return;
    }

    // ... closer/intermediate handling unchanged
}
```

This means `validate_args_against_spec` and `validate_argument_order` stay but are ONLY reachable via user config. They are NOT a safety net for builtins (those all have extracted rules).

**Revised Phase 3 approach:** Don't delete `validate_args_against_spec`/`validate_argument_order`. Instead, make them unreachable for builtin tags (which all have extracted rules) but still available for user-config-only tags. This is not a "safety net" — it's a distinct feature (user-defined argument specs for tags that extraction can't handle).

#### 5. Update test fixtures

Remove or update any tests that construct `TagArg` specs for builtin tags. Keep tests that verify user-config `TagArg` behavior.

#### 6. Update AGENTS.md operational notes

Remove/update notes referencing the old `args`-based validation system. Add notes about `ExtractedRuleViolation` (S117) and the extraction-derived completions pipeline.

### Success Criteria

#### Automated Verification:
- [ ] `cargo test -q` — all tests pass
- [ ] `cargo clippy -q --all-targets --all-features -- -D warnings`
- [ ] No dead code warnings for removed items

**Implementation Note:** After completing this phase and all automated verification passes, pause here for manual confirmation before proceeding to Phase 6.

---

## Phase 6: Corpus Template Validation Tests

### Overview

This is the proof. Port the prototype's `test_corpus_templates.py` and `test_real_templates.py` to Rust. Validate actual templates from the corpus against extracted rules and assert zero false positives. This is the test that proves the entire M1-M8 pipeline works end-to-end.

### What the Prototype Tests

From `template_linter/tests/test_corpus_templates.py`:
1. For each corpus entry (Django versions, third-party packages, project repos):
   - Extract rules from the entry's own templatetags
   - Find all `.html` templates in the entry
   - Validate each template against the extracted rules
   - Assert zero errors (false positives)
2. Version-aware: Django 4.2 templates validated against Django 4.2 rules
3. Entry-local: Wagtail templates validated against Wagtail's own extracted rules + Django builtins
4. Known-invalid templates are explicitly tested to FAIL

From `template_linter/tests/test_real_templates.py`:
1. Django's shipped templates (contrib/admin) validated against Django's own extracted rules
2. Zero false positives under strict mode (unknown tags/filters reported)

### Changes Required

#### 1. Template corpus validation test

**File:** `crates/djls-server/tests/corpus_templates.rs` (new integration test)

This test needs access to:
- `djls-extraction` (with `parser` feature) — for extracting rules from corpus
- `djls-semantic` — for validation
- `djls-templates` — for parsing templates
- `djls-server` — for `DjangoDatabase` and `compute_tag_specs`

```rust
//! Corpus-scale template validation tests.
//!
//! These tests validate actual templates from the corpus against
//! extracted rules, proving zero false positives end-to-end.
//!
//! This is the Rust port of the prototype's test_corpus_templates.py
//! and test_real_templates.py — the ultimate proof the system works.
//!
//! # Running
//!
//! ```bash
//! # First, sync the corpus:
//! just corpus-sync
//!
//! # Then run corpus template validation:
//! cargo test -p djls-server corpus_templates -- --nocapture
//! ```

/// Get corpus root (same logic as extraction corpus tests)
fn corpus_root() -> Option<PathBuf> { ... }

/// Find all HTML/txt template files in a directory tree
fn find_templates(root: &Path) -> Vec<PathBuf> { ... }

/// Build tag specs from extraction of a corpus entry's templatetags
fn build_specs_for_entry(entry: &Path) -> TagSpecs { ... }

/// Validate a single template file against given specs.
/// Returns list of validation errors.
fn validate_template_file(
    content: &str,
    specs: &TagSpecs,
) -> Vec<ValidationError> { ... }
```

**Test: Django shipped templates validate cleanly**

```rust
#[test]
fn test_django_shipped_templates_zero_false_positives() {
    let Some(root) = corpus_root() else {
        eprintln!("Corpus not available. Run `just corpus-sync`.");
        return;
    };

    // Find all Django version entries
    let django_packages = root.join("packages/Django");
    if !django_packages.exists() { return; }

    for version_dir in sorted_version_dirs(&django_packages) {
        let version = version_dir.file_name().unwrap().to_string_lossy();

        // Extract rules from THIS Django version's source
        let specs = build_specs_for_django_entry(&version_dir);

        // Find shipped templates (contrib/admin/templates, forms/templates)
        let templates = find_django_shipped_templates(&version_dir);

        let mut failures: Vec<(PathBuf, Vec<String>)> = Vec::new();

        for template_path in &templates {
            let content = std::fs::read_to_string(template_path)
                .unwrap_or_default();

            let errors = validate_template_file(&content, &specs);

            // Filter to only argument validation errors (S117, S104-S107)
            // Block structure (S100-S103) and expression validation (S114)
            // are also validated but tracked separately
            if !errors.is_empty() {
                failures.push((
                    template_path.clone(),
                    errors.iter().map(|e| e.to_string()).collect(),
                ));
            }
        }

        assert!(
            failures.is_empty(),
            "Django {} shipped templates have false positives:\n{}",
            version,
            format_failures(&failures),
        );

        eprintln!(
            "✓ Django {} — {} templates validated, zero false positives",
            version, templates.len()
        );
    }
}
```

**Test: Third-party package templates validate cleanly**

```rust
#[test]
fn test_third_party_templates_zero_false_positives() {
    let Some(root) = corpus_root() else { return; };

    let packages_dir = root.join("packages");
    if !packages_dir.exists() { return; }

    for entry_dir in sorted_entry_dirs(&packages_dir) {
        let entry_name = entry_dir.file_name().unwrap().to_string_lossy();

        // Skip Django (tested separately above)
        if entry_name.starts_with("Django") { continue; }

        // Extract rules from entry's own templatetags + Django builtins
        let specs = build_specs_for_third_party_entry(&entry_dir, &root);

        let templates = find_templates(&entry_dir);
        if templates.is_empty() { continue; }

        let mut failures = Vec::new();

        for template_path in &templates {
            // Skip known test/doc templates that are intentionally invalid
            if is_excluded_template(template_path) { continue; }

            let content = std::fs::read_to_string(template_path)
                .unwrap_or_default();
            let errors = validate_template_file(&content, &specs);

            if !errors.is_empty() {
                failures.push((template_path.clone(), errors));
            }
        }

        // For third-party packages without load resolution, we can't do
        // strict unknown-tag checking. Only check argument validation
        // errors (false positives from extracted rules).
        let arg_failures: Vec<_> = failures.iter()
            .filter(|(_, errs)| errs.iter().any(|e|
                matches!(e,
                    ValidationError::ExtractedRuleViolation { .. }
                    | ValidationError::TooManyArguments { .. }
                    | ValidationError::MissingRequiredArguments { .. }
                )
            ))
            .collect();

        assert!(
            arg_failures.is_empty(),
            "{} templates have argument validation false positives:\n{}",
            entry_name, format_failures_ref(&arg_failures),
        );

        eprintln!(
            "✓ {} — {} templates validated",
            entry_name, templates.len()
        );
    }
}
```

**Test: Repo templates (Sentry, NetBox) validate cleanly**

```rust
#[test]
fn test_repo_templates_zero_false_positives() {
    let Some(root) = corpus_root() else { return; };

    let repos_dir = root.join("repos");
    if !repos_dir.exists() { return; }

    // Similar structure to third-party test
    // Extract rules from repo's own templatetags + Django builtins
    // Validate all templates
    // Assert zero argument validation false positives
}
```

**Test: Known-invalid templates produce expected errors**

```rust
#[test]
fn test_known_invalid_templates_caught() {
    // Specific templates that SHOULD produce errors
    // (intentionally invalid syntax, unknown tags, etc.)
    // Asserts that the validation system catches real problems
}
```

#### 2. Test infrastructure helpers

The test needs a lightweight database that:
- Takes a `TagSpecs` (built from extraction)
- Provides `validate_nodelist` (from `djls-semantic`)
- Parses templates (from `djls-templates`)

This can be a test-only `CorpusTestDatabase` similar to the `TestDatabase` pattern used in `arguments.rs` and `filter_arity.rs` tests.

#### 3. Template exclusion list

Port the prototype's exclusion list from `conftest.py`:

```rust
/// Templates known to be intentionally invalid or non-Django syntax.
const EXCLUDED_TEMPLATE_SUFFIXES: &[&[&str]] = &[
    // AngularJS templates under static/**/templates
    &["geonode", "static", "geonode", "js", "templates", "cart.html"],
    // Known-invalid upstream templates
    &["babybuddy", "templates", "error", "404.html"],
    &["src", "sentry", "templates", "sentry", "emails", "onboarding-continuation.html"],
];
```

#### 4. Add to Justfile

**File:** `Justfile`

```just
# Run corpus template validation tests (requires corpus-sync first)
corpus-validate:
    cargo test -p djls-server corpus_templates -- --nocapture
```

### Success Criteria

#### Automated Verification:
- [ ] `cargo test -p djls-server corpus_templates` — all tests pass (with corpus synced)
- [ ] Django 4.2 shipped templates: zero false positives
- [ ] Django 5.1 shipped templates: zero false positives
- [ ] Django 5.2 shipped templates: zero false positives
- [ ] Django 6.0 shipped templates: zero false positives
- [ ] Wagtail templates: zero argument validation false positives
- [ ] django-allauth templates: zero argument validation false positives
- [ ] django-crispy-forms templates: zero argument validation false positives
- [ ] django-debug-toolbar templates: zero argument validation false positives
- [ ] django-compressor templates: zero argument validation false positives
- [ ] Sentry templates: zero argument validation false positives (excluding known-invalid)
- [ ] NetBox templates: zero argument validation false positives
- [ ] Known-invalid templates produce expected errors
- [ ] Tests skip gracefully when corpus not synced (no failures)

#### Manual Verification:
- [ ] Review corpus test output showing template counts per entry
- [ ] Verify the corpus includes meaningful template coverage (not just 1-2 files per entry)

**Implementation Note:** This is the final phase. All corpus tests passing is THE proof that M8 is complete.

---

## Testing Strategy Summary

### Three Tiers

| Tier | Location | Gating | Purpose |
|------|----------|--------|---------|
| **T1: Unit** | Inline `#[cfg(test)]` in each module | Always | Per-function, per-variant correctness |
| **T2: Integration** | `crates/djls-extraction/tests/golden.rs` | Always | Extraction output stability |
| **T3: Corpus** | `crates/djls-extraction/tests/corpus.rs` + `crates/djls-server/tests/corpus_templates.rs` | Corpus synced | **THE PROOF** — zero false positives at scale |

### Key Regression Tests

- `{% for item in items football %}` → error (Phase 3)
- `{% for item in items %}` → no error (Phase 3)
- `{% for item in items reversed %}` → no error (Phase 3)
- `{% autoescape on %}` → no error (Phase 3)
- `{% autoescape %}` → error (Phase 3)
- `{% if and x %}` → S114 error (unchanged, expression validation)
- `{{ x|truncatewords }}` → S115 error (unchanged, filter arity)
- `{% endblock content %}` → no error (Phase 3)
- `{% csrf_token extra %}` → depends on extraction output
- Django admin templates → zero false positives (Phase 6)

### MaxArgCount Semantics — Critical Detail

The `MaxArgCount{max:3}` condition from extraction needs careful interpretation. Looking at the `for` tag extraction:

```yaml
- condition: MaxArgCount { max: 3 }
  message: "'for' statements should have at least four words"
```

This means: "error when `len(split_contents()) <= 3`" (i.e., 3 or fewer words including tag name). The condition fires when the count is AT or BELOW the max — this represents a minimum. The naming is from the extraction side (it's the MAX value that triggers the error guard `if len(bits) < 4`).

The evaluator must implement: `split_len <= max` → violated.

Verify by cross-referencing with the original Django source:
```python
if len(bits) < 4:  # i.e., <= 3
    raise TemplateSyntaxError("'for' statements should have at least four words")
```

Extracted as `MaxArgCount{max:3}` = "error when len ≤ 3". Correct.

---

## Performance Considerations

- Rule evaluation is O(n) per tag where n = number of extracted rules (typically 1-5). Negligible compared to parsing.
- Argument extraction adds one AST walk per registration during extraction. Cached by Salsa.
- Corpus template tests are slow (hundreds of files) — gated behind corpus sync, not run in CI by default.

## Migration Notes

- **User config `args` in `djls.toml`:** Still works. Tags defined via user config with `args` are validated by the old `validate_argument_order` path (preserved for this purpose only). This is an escape hatch for tags that extraction can't handle.
- **Diagnostic code change:** New code S117 (`ExtractedRuleViolation`) replaces S104-S107 for extraction-validated tags. Users with `diagnostics.ignore = ["S104"]` may need to also add "S117". Document this in release notes.
- **Snippet quality:** May differ slightly from hand-crafted versions (different argument names, missing optional modifiers). This is acceptable — extraction-derived names are authoritative.

## References

- Charter: [`.agents/charter/2026-02-05-template-validation-port-charter.md`](../charter/2026-02-05-template-validation-port-charter.md)
- M5 Plan: [`.agents/plans/2026-02-05-m5-extraction-engine.md`](2026-02-05-m5-extraction-engine.md)
- M6 Plan: [`.agents/plans/2026-02-05-m6-rule-evaluation.md`](2026-02-05-m6-rule-evaluation.md)
- Prototype corpus tests: `template_linter/tests/test_corpus_templates.py`, `template_linter/tests/test_real_templates.py`
- Working extraction→evaluation model: `crates/djls-semantic/src/filter_arity.rs`

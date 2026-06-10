# How ruff and ty statically resolve Python source

Date: 2026-06-10
Subject: a deep dive into the machinery ruff and ty use to understand Python source without running it — what exists at each tier of sophistication, what each tier costs and can answer, and where each project extracts concrete *values* (not types) from source. This is the "really hard part" of static Django discovery: getting `INSTALLED_APPS`, `TEMPLATES`, settings layering, and templatetag registrations out of project Python code statically.

All citations are into `reference/ruff` (checkout `8b79528100`, 2026-06-09). Companion document: [research.md](research.md) (PR #606/#626 review and house-cleaning sequence). Note on layout: in this revision ty's semantic index was split out of `ty_python_semantic` into its own crate, `ty_python_core` — older ty docs refer to `ty_python_semantic/src/semantic_index/`.

## TL;DR

There are four tiers of static Python understanding in the ruff/ty stack, and the finding that matters most is this: **value extraction never lives in the big tiers.** Even inside ty's 137k-line type checker, every time the authors need concrete runtime values — `__all__` names, namespace-package idioms, namedtuple fields, TypedDict keys, enum members — they write a small bounded AST recognizer (200–900 lines, closed shape list, honest bail) instead of asking the type system. The type system itself discards the values DLS needs: the public type of `INSTALLED_APPS = ["a", "b"]` in ty is `list[str]` — the strings are gone.

The single most important file for DLS is **`ty_python_semantic/src/dunder_all.rs` (444 lines)**: a Salsa-tracked, cycle-safe, per-file statement walker that solves almost exactly the `INSTALLED_APPS` problem shape — assignment, `+=`, `.append()`/`.extend()`/`.remove()`, cross-module composition (`__all__ += other.__all__` recursing into the other module's query), star imports, and statically evaluated `if` branches — with an `invalid` latch that returns `None` rather than ever guessing.

| Tier | Machinery | Size | What it answers |
|---|---|---|---|
| 0 | `ruff_python_parser` + `ruff_python_ast` | ~48k | source → full AST, always (errors as list, not failure) |
| 1 | syntax-only recognizers | 0.1–1k each | "is this one of N known idioms?" + literal values inside them |
| 2 | ruff's single-pass semantic model | ~13k | per-file name→qualified-name, one-hop assigned values, branch bookkeeping |
| 3 | ty's semantic index | ~17k | flow-sensitive use-def, reachability, per-scope Salsa firewalls |
| 4 | ty's type inference | ~137k | types (literals propagate, but collections widen — values lost) |

DLS needs tier 0 + tier 1 recognizers + a few tier-2 techniques (qualified names, one-hop assignment lookup) + two ideas downscaled from tier 3/4 (three-valued branch truthiness; the star-import/exported-names split). It does not need a use-def map, a constraint solver, or inference.

---

## Tier 0: the parser and AST (shared foundation)

**Entry API.** `parse_module(source) -> Result<Parsed<ModModule>, ParseError>` at `crates/ruff_python_parser/src/lib.rs:113`; the lossy `parse_unchecked` (`lib.rs:290`) returns `Parsed` *unconditionally*. `Parsed<T>` (`lib.rs:304-310`) carries `{ syntax, tokens, errors, unsupported_syntax_errors }` — **the parser always produces a complete AST plus an error list**. Recovery is built into the hand-written recursive-descent parser (`parser/recovery.rs`, recovery loop at `parser/mod.rs:530-565`); missing expressions are synthesized as empty `ExprName` placeholders. Caveat: the ruff *linter* only runs AST rules on files with valid syntax (`ruff_linter/src/linter.rs:195-196`); ty and the formatter consume the resilient AST.

**What the AST carries.** Every node has a `TextRange` and node index; no trivia in the tree. Node shapes relevant to value extraction (all in `ruff_python_ast/src/generated.rs`):

- `StmtAssign` (:9033), `StmtAugAssign` (:9043), `StmtAnnAssign` (:9054)
- `StmtImportFrom` (:9173) — relative-import dots are a `u32` `level`, not text
- `ExprBinOp` (:9319) — `BASE_DIR / "templates"` is `BinOp { op: Div }`
- `ExprStringLiteral` (:9502) — implicit concatenation (`"a" "b"`) is modeled *inside* the node; `StringLiteralValue::to_str()` (`src/nodes.rs:1448-1453`) returns the already-joined string. Free win for extractors.
- `ExprList`/`ExprTuple` (:9593/:9603), `ExprDict` (:9361, `**spread` = `None` key), `ExprCall` (:9459)
- Integers parse to real values (`src/int.rs`, checked accessors like `Int::as_u8()`)

Sizes: `ruff_python_parser` 20,385 LOC; `ruff_python_ast` 27,719 (10.9k generated).

---

## Tier 1: syntax-only recognizers (the workhorses)

The pattern: a closed list of statement/expression shapes, literal-value checks inside them, and an honest bail on anything else. Three canonical examples:

**The legacy namespace-package recognizer** — `ty_module_resolver/src/resolve.rs:1505-1774` (~270 lines). Recognizes exactly three statement shapes (`__path__ = pkgutil.extend_path(__path__, __name__)` and two variants), checking the *string argument values* `"pkgutil"`/`"pkg_resources"` (:1685, :1762) and argument positions. Top-level statements only (:1603-1606). The philosophy comment at :1515-1521 is the design thesis:

> "This is all syntax-only analysis so it could be fooled… The benefit is speed and avoiding circular dependencies between module resolution and semantic analysis… if you write slightly different syntax we will fail to detect the idiom, but hey, this is better than nothing!"

**Typeshed VERSIONS parsing** — `ty_module_resolver/src/typeshed.rs:163-289` (~130 lines of parser): line-oriented `module: 3.8-3.10` format, hard errors on malformed lines. Bounded-parser shape applied to a sidecar file.

**ruff's version-block evaluator (UP036)** — `ruff_linter/src/rules/pyupgrade/rules/outdated_version_block.rs` (497 lines). The full model for "statically resolve a branch, keep the live arm":

1. Recognize the guard: `resolve_qualified_name(...)` must be exactly `["sys", "version_info"]` (:460-467).
2. Extract the literal comparator: `extract_version` (:439-452) — every tuple element must be an int literal; one non-literal → `None` → the rule silently gives up (:115-117).
3. Evaluate: element-wise bounded comparison against the configured target version (:213-264), `Err` not panic on out-of-range.
4. Rewrite the `if`/`elif`/`else` keeping only the live arm (:277-436).

---

## Tier 2: ruff's single-pass semantic model (~13k LOC)

**Crate:** `ruff_python_semantic` (9,517 LOC: `model.rs` 2,883; `binding.rs` 852; `scope.rs` 305; `analyze/` ≈3,483) plus the builder traversal in `ruff_linter/src/checkers/ast/mod.rs` (3,775). One linear pass in Python evaluation order (deferred visits for function bodies/string annotations, mod.rs:3224-3232), no fixpoints, no Salsa, rebuilt per file per run — and that's fast enough for ruff's performance brand.

**Bindings and scopes.** `Binding` (`binding.rs:19-35`) links a name to its defining statement (`Binding::statement()`, :272-275 — what makes "what was assigned to this name" answerable). `BindingKind` (:523-685) has 19 variants; the three import kinds each carry a **pre-computed qualified name** (:492-521). `Scope::add` records shadow chains (`scope.rs:78-85`): `get` returns the latest binding, `get_all` (:104-108) walks every shadowed predecessor — how consumers see *all* conditional `__all__` definitions, not just the last. Star imports are recorded only as a scope-level marker (`StarImport`, :129-137); names are never materialized.

**`resolve_qualified_name`** (`model.rs:1017-1131`) — the local-name → dotted-path engine: `import django.db as d; d.models.Model` → `django.db.models.Model`; `from sys import version_info as v` → uses of `v` resolve to `sys.version_info`; relative imports expand against the module's own path (`from_relative_import`, `ruff_python_ast/src/helpers.rs:1068-1098` — requires a detected package root). A name bound by plain assignment resolves to `None`. The inverse, `resolve_qualified_import_name` (:1145-1252), answers "what local name refers to module member X here."

**One-hop value lookup.** `analyze::typing::find_assigned_value` / `find_binding_value` (`analyze/typing.rs:1209-1268`): symbol → binding → the assigned `Expr`, handling plain/annotated assignment, walrus, `with … as`, and positional destructuring. **One hop, no transitive chasing, no aug-assign, no mutation tracking** — by design. `resolve_assignment` (:1168-1197) qualifies the callee of a call-RHS: `register = template.Library()` then `register.tag` → `["django","template","Library","tag"]` — **directly the primitive needed for `@register.tag` detection**.

**Flow sensitivity: bookkeeping, not dataflow.** Two mechanisms — traversal order (later bindings shadow earlier; no branch merging) and `BranchId` per node with `same_branch`/`dominates` prefix checks (`model.rs:1726-1790`). A read after `if/else` sees the textually-last binding plus the shadow chain.

**Hard limits** (each verified in code): single-file only (`check_ast` builds one model per file; `from .base import *` → opaque `StarImport` marker, lookups through it return `ReadResult::WildcardImport`, `model.rs:674-681`); no mutation/alias tracking; no path-sensitive merging; dynamic constructs → `Unknown`.

### The `__all__` extractor — ruff's version

~250 LOC total: `ruff_python_semantic/src/model/all.rs` (206) plus the hook in `Checker::handle_node_store` (`checkers/ast/mod.rs:2784-2852`). Statement dispatch covers `Assign`/`AnnAssign`/`AugAssign`; a hand-rolled loop linearizes `+`-chains (`all.rs:110-134`); per-expression match arms (:151-204) accept list/tuple literals, `list()`/`tuple()` wrapping (callee verified as the builtin), walrus, and *skip-without-dying* on comprehensions and references to other modules' `__all__`. Elements must be string literals; a bad element sets `INVALID_OBJECT` but **extraction continues** (:84-98) — partial results with flags, never a full bail. Each `__all__` statement gets its own `Export` binding; conditional definitions coexist on the shadow chain and consumers iterate all of them (`visit_exports`, mod.rs:3236-3294).

What ruff deliberately does not handle: `.append()`/`.extend()`/`.remove()` — method calls never reach the store hook. The existence proof is rule PYI056 (`flake8_pyi/rules/unsupported_method_call_on_all.rs:60-83`), which flags exactly those calls and tells users to write `+=` instead, "known to be supported by all major type checkers." When a static tool can't see an idiom, ruff's answer is sometimes *tell the user to write the analyzable form*.

### flake8-django: how ruff answers Django questions today

`ruff_linter/src/rules/flake8_django/` (845 LOC total). Everything reduces to qualified-name matching:

```rust
// rules/flake8_django/helpers.rs:6-13
pub(super) fn is_model(class_def: &ast::StmtClassDef, semantic: &SemanticModel) -> bool {
    analyze::class::any_qualified_base_class(class_def, semantic, |qualified_name| {
        matches!(qualified_name.segments(), ["django", "db", "models", "Model"])
    })
}
```

The base-class walk (`analyze/class.rs:16-79`) recurses through *locally defined* intermediate bases with cycle protection; bases imported from other project files are opaque. DJ013 (`non_leading_receiver_decorator.rs:55-82`) matches decorator calls resolving to `["django","dispatch","receiver"]` — the template for `@register.tag`. DJ006/DJ007 match `Meta.fields` against the literal string `"__all__"`. All Django rules early-return unless an `import django…` was seen (`Modules::DJANGO` bitflag, `model.rs:1514-1548`) — a cheap relevance gate worth copying.

---

## Tier 3: ty's semantic index (~17k LOC)

**Crate:** `ty_python_core` (17,393 LOC: `builder.rs` 4,820; `use_def.rs` + `place_state.rs` 3,039; `definition.rs` 1,765; `reachability_constraints.rs` 488; `re_exports.rs` 433).

**Entry query** — one Salsa tracked query per `File` (`lib.rs:69-76`): `semantic_index(db, file)` loads `parsed_module` and runs the builder. It's `no_eq` (always "changed"); incrementality comes from per-scope **firewall queries** `place_table(db, scope)` (:83-89) and `use_def_map(db, scope)` (:96-102), whose `Arc` contents Salsa compares — edits in one scope don't invalidate consumers of another.

**What one pass produces:** per-scope place tables (symbols `x` plus members `x.y`/`x[0]`); the scope tree; `Definition`s (Salsa tracked structs, the universal unit for bindings *and* declarations — `DefinitionKind` at `definition.rs:914-940` covers every binding form); standalone `Expression`s marked for independent inference; use-def maps; dense per-use IDs.

**AST references across query boundaries.** `AstNodeRef<T>` (`ast_node_ref.rs:35-101`) stores only a `NodeIndex`; dereferencing requires a live `ParsedModuleRef`. In tracked structs the node field is `#[tracked] #[no_eq]` so a re-parse doesn't change the struct's identity (:18-34). `parsed_module` itself is `lru=200` with a clearable payload that transparently **re-parses after garbage collection** (`ruff_db/src/parsed.rs:33-40, 117-137`). This is how ty keeps ASTs out of long-lived memory without dangling references.

**Flow sensitivity — the use-def map** (module doc `use_def.rs:1-240`). The builder keeps live binding/declaration bit-sets per place; control flow is snapshot/restore/merge. The `if` visitor (`builder.rs:3099-3191`): visit test → snapshot → record narrowing + reachability constraints → visit body → restore falsy state with negated constraints → visit else → `flow_merge`. After `if flag: x = 3 else: x = 4`, a use of `x` has **two live bindings** and infers the union. Loops use synthesized `LoopHeader` definitions with fixpoint widening (`lib.rs:104-191`).

**Static condition evaluation — record now, decide later.** Branch conditions become `Predicate`s (`predicate.rs:75-134`); reachability constraints are stored as a **Ternary Decision Diagram** — a BDD with an added `Ambiguous` leaf (`reachability_constraints.rs:13-137`; blow-up bound 512k interior nodes, beyond which everything degrades to `AMBIGUOUS`). At check time (`ty_python_semantic/src/reachability.rs`, Kleene three-valued logic, doc :1-194), each predicate is resolved by **inferring the type of the test expression and asking for its boolean** (:1065-1073). There is no special version-check recognizer — see Tier 4. Unknown conditions (`if DEBUG:`) evaluate `Ambiguous` → both branches' bindings stay live → uses see the union. **ty never guesses; it unions.** This is the precise analog of env-dependent Django settings branches.

---

## Tier 4: ty's type inference (~137k LOC)

**Crate:** `ty_python_semantic` (137,406 LOC; `types/` 126,784; all inference 31,944; `types/infer/builder.rs` alone 11,326). Inference runs as Salsa queries at four granularities — scope, statement, definition, expression (`types/infer.rs:1-44`) — so resolving one imported symbol never infers the whole exporting module (:15-19). Every query declares `cycle_initial`/`cycle_fn` fixpoint recovery seeded with `Type::divergent` (:39-44, 98-107).

**Literals propagate; collections widen.** Literal values live in the type lattice: `Type::LiteralValue` with String/Bytes/Int/Bool/Enum kinds (`types.rs:896, 1694-1700`), strings capped at 4,096 bytes (`infer/builder.rs:350`), ints at `i64`. **Tuples keep per-element types** (`infer_tuple_expression`, `infer/builder.rs:6419-6545`): `(1, "a")` is `tuple[Literal[1], Literal["a"]]`. **Lists do not**: `infer_list_expression` → `infer_collection_literal` (:6547-6567, 6770-7180) solves `list[T]` with literal element types *promoted* — `["Sheet1"]` is `list[str]` (mdtest/bidirectional.md:510). `.append()` calls constrain only *empty* unannotated literals (statement-level "constraining uses," `builder.rs:3895-3982`), and they constrain the element *type*, not values.

**The key question answered.** For `INSTALLED_APPS = ["a", "b"]`, ty's public type is `list[str]` — no query hands back the strings. (Element literals exist transiently in per-region expression-type tables, but the public API discards them.) Had Django convention been tuples, `tuple[Literal["a"], Literal["b"]]` would preserve everything — but it's lists. **A type checker is the wrong API for value extraction even when you have one.** ty's authors demonstrably agree: every time *they* need values, they write a Tier-1 recognizer.

**Why `sys.version_info >= (3, 12)` needs zero recognizer code.** `sys.version_info` is special-cased in symbol lookup to a heterogeneous tuple of literal ints (`place.rs:1149-1152`, `tuple.rs:1524-1550`); tuple comparison folds lexicographically (`comparisons.rs:963-1091`); int-literal comparison folds to `Literal[True/False]` (:398-405). Likewise `sys.platform` → a string literal when a target platform is configured (`place.rs:1154-1161`), and `TYPE_CHECKING` is forced to `Literal[True]` (`infer/builder.rs:4601-4627`). The "extractor" is the type system itself — value propagation through the lattice. That generality is what the 137k lines buy; it is the second style of value handling, and it is exactly the style DLS should *not* attempt.

**Constant folding in the type domain** (values computed, not just preserved): checked int arithmetic widening to `int` on overflow (`binary_expressions.rs:556-672`); string/bytes concatenation and repetition under the 4,096 cap (:674-746); f-string folding when all interpolations are literal (`infer/builder.rs:6314-6383`); literal subscript/slice — `"value"[1:3]` → `Literal["al"]` (`subscript.rs:574-700`).

---

## Cross-module resolution (how ty follows imports)

- **`from x import y`**: `ModuleName::from_import_statement` (`ty_module_resolver/src/module_name.rs:320`; relative logic :489) → `resolve_module` (`resolve.rs:57`) → `imported_symbol(db, file, name, …)` (`ty_python_semantic/src/place.rs:422-488`), which looks the name up in the exporting module's global scope **at end-of-scope** (`ConsideredDefinitions::EndOfScope` — the deliberate "module finished executing" simplification, `use_def.rs:145-161`). Public type = declared if declared, else inferred from end-of-scope live bindings (`place_by_id`, `place.rs:990-1058`). Module-level globals deliberately expose raw inferred literal types (`place.rs:1029-1057`).
- **`from .base import *`** — two-part machinery: (1) `exported_names(db, file)` (`ty_python_core/src/re_exports.rs:34-50`, 433 lines) — a separate, *syntax-only* Salsa query collecting a module's global-scope binding names, cycle-safe via an empty-default fixpoint seed; (2) `StarImportPlaceholderPredicate` (`predicate.rs:226-293`) — each exported name gets a definition in the importing file guarded by a predicate modeled literally as `if <placeholder>: from a import A`, resolved at check time by looking the symbol up in the exporting module and consulting `dunder_all_names` (`reachability.rs:1135-1174`).
- **Unresolved imports**: diagnostic + `Type::unknown()` for the bound name (`infer/builder/imports.rs:537-594`); gradual propagation, nothing panics.

---

## The values catalog

Every site in both stacks that extracts concrete runtime values from Python source. The invariant across all of them: *small bounded recognizer; fixed shape list; honest bail.*

### ty

| What | Citation | Shapes | Bail behavior | Size |
|---|---|---|---|---|
| `__all__` names | `ty_python_semantic/src/dunder_all.rs` | `=`, annotated, `+=`, `.append/.extend/.remove`, `+= other.__all__` (recurses cross-module), `from m import __all__`, `import *`, statically-evaluated `if` branches; walks `for`/`while`/`try`, never nested scopes | one unrecognized idiom → `invalid` latch → whole query returns `None` (:199-209) | 444 |
| namespace-package idiom | `ty_module_resolver/src/resolve.rs:1505-1774` | exactly 3 statement shapes incl. literal `"pkgutil"`/`"pkg_resources"` args | not recognized → not a namespace package | ~270 |
| `sys.version_info` comparisons | `place.rs:1149-1152` + `tuple.rs:1524-1550` + `comparisons.rs:963-1091` | literal int tuple comparisons (via lattice, no recognizer) | non-literal → `Ambiguous` branch | ~30 specific |
| typeshed VERSIONS | `ty_module_resolver/src/typeshed.rs:163-289` | `module: 3.8-3.10` lines | hard parse error | ~130 |
| `namedtuple`/`NamedTuple` fields | `infer/builder/named_tuple.rs` | string literal (split on space/comma), list/tuple of literals, `rename=True` semantics | `NamedTupleSpec::unknown` (:473-476) | 773 |
| functional `TypedDict` keys | `typed_dict.rs:564-616` | dict literal with string-literal keys; literal `total=False` | default schema | ~75 |
| functional `Enum` members | `infer/builder/enum_call.rs` | string / sequence / pairs / dict member specs | tri-state `Known/Unknown/Invalid` (:29-37) | 915 |
| enum class member values | `enums.rs:597-830` | end-of-scope bindings; literal values kept for alias detection, `auto()` simulated | empty metadata seed | ~230 |
| `@deprecated` message | `known_instance.rs:453-458` | string literal | `None` | tiny |
| `dataclasses.field(alias=…)` | `known_instance.rs:462-476` | string literal | `None` | tiny |
| `typing.Literal[...]` grammar | `infer/builder/type_expression.rs:2458-2567` | closed literal grammar incl. unary ± | diagnostic + `Unknown` | ~110 |
| string annotations | `types/string_annotation.rs` + `ruff_db/src/parsed.rs:53-91` | re-parse the string with the real parser | diagnostic | ~200 |

### ruff linter

| What | Citation | Shapes | Bail behavior |
|---|---|---|---|
| `__all__` names | `ruff_python_semantic/src/model/all.rs:75-205`; hook `checkers/ast/mod.rs:2808-2852` | `=`/`(…)`/annotated/`+=`/`+`-chains/`list()`-`tuple()` wrap/walrus; comprehensions and foreign `__all__` skipped | partial-with-flags (`INVALID_FORMAT`/`INVALID_OBJECT`); `.append` invisible → PYI056 tells the user |
| `__slots__` members | `pylint/rules/non_slot_assignment.rs:104-288` | `=`, annotated, `+=`; tuple/list/set elements, dict keys; string literals only | any non-literal → entire result discarded (`return vec![]`) |
| version comparison tuples | UP036 `pyupgrade/rules/outdated_version_block.rs:439-452` | `<,<=,>,>=` vs int-literal tuple; `==/!=` vs major | `None` → rule skips |
| `sys.version` index/slice constants | `flake8_2020/rules/{subscript.rs:186-199, compare.rs:246-255}` | literal int subscripts/slices | skip |
| %-format / `.format` template structure | `ruff_python_literal/src/{cformat,format}.rs`; `pyflakes/{cformat.rs:10-50, format.rs:26-80}` | literal format strings (incl. implicit concat) | parse error → diagnostic, no analysis |
| env-var names | SIM112 `flake8_simplify/rules/ast_expr.rs:136-227` | string literal in `os.environ[…]`/`.get()`/`os.getenv()` | skip |
| `getattr`/`setattr` constant attrs | B009/B010 `flake8_bugbear/rules/getattr_with_constant.rs:81-94` | string-literal 2nd arg, identifier-validated | skip |
| Django `Meta.fields = "__all__"` | DJ007 `flake8_django/rules/all_with_model_form.rs:62-96` | string/bytes literal | skip |
| Django field kwargs (`null=True`) | DJ001 `flake8_django/rules/nullable_model_string_field.rs:83-110` | literal `True` kwargs on resolved `django.db.models.*Field` calls | skip |
| qualified names (the meta-extractor) | `model.rs:1017-1131` | all import forms, relative w/ package root, builtins, local defs | `None` for assignments, star imports |
| one-hop assigned value | `analyze/typing.rs:1209-1329` | `=`, annotated, walrus, `with…as`, positional destructuring | `None` (no aug-assign, loops, attributes) |
| constant truthiness | `ruff_python_ast/src/helpers.rs:1393-1557` | literals, containers, f-strings, builtin-initializer calls | `Unknown` |

Bail-out styles observed, in increasing tolerance: whole-result discard (`__slots__`), invalid-latch returning `None` (ty `__all__`), silent skip (UP036), partial-with-flags (ruff `__all__`), defer-to-`Unknown` (truthiness), and tell-the-user (PYI056).

---

## The economics

| Layer | LOC | Character |
|---|---|---|
| `ruff_python_parser` | 20,385 | shared, done, reusable |
| `ruff_python_ast` | 27,719 | shared, done, reusable |
| ruff semantic model + checker traversal | ~13,300 | one linear pass, per-file, no fixpoints, no Salsa |
| `ty_python_core` (semantic index) | 17,393 | flow-sensitive, Salsa per-scope firewalls, TDD reachability |
| `ty_python_semantic` (types) | 137,406 | region inference, fixpoints, constraint solving |
| `ty_module_resolver` | 9,249 | search paths, module resolution, recognizers |
| **all of ty's value extractors combined** | **~2,800** | bounded recognizers, honest bails |

The semantic index alone is ~8× the size of all of ty's value extractors combined. The 137k-line tier exists to make *propagation* general; *extraction* stays tiny everywhere.

---

## What DLS should build (the verdict)

DLS's settings/app/templatetag extraction belongs at the `dunder_all_names` tier. Concretely:

1. **A per-name statement walker with an invalid latch** (the `dunder_all.rs` pattern). Enumerate supported shapes for `INSTALLED_APPS`/`TEMPLATES`: assignment, annotated assignment, `+=`, `+`-chains, `.append()`/`.extend()`/`.insert(0, …)`, list/tuple literals with string-literal elements. One unrecognized idiom → return `None`/partial-with-reason for that fact — never a wrong answer. Last-assignment-wins along the walked statement order replaces a use-def map (exactly what ty's `__all__` collector does — `update_origin` clears on re-assignment, `dunder_all.rs:57-66`). PR #606's `settings_facts.rs` had this idea but unbounded: `.sort()`/`.reverse()` emulation and path-returning-function imports are past the value/cost knee — neither reference project models anything like them.

2. **A per-file Salsa query with a cheap cycle seed** (`cycle_initial=None`), recursing into other settings modules *through the same query* for `from .base import *` layering — cycle-safe for free, exactly how `dunder_all_names` recurses for `__all__ += other.__all__`.

3. **A syntax-only `exported_names`-style query** (`re_exports.rs:34`) for star-import layering: "what names does this settings module bind at top level," with later assignments shadowing star-imported values via ordered walk. ty's placeholder-predicate machinery exists for boundness *precision* DLS doesn't need.

4. **Three-valued branch truthiness, downscaled** (no TDD, no inference): a tiny `evaluate_test_expr` recognizing `True`/`False` literals, `TYPE_CHECKING`-style constants, optionally `sys.version_info` literal comparisons (UP036's ~40-line evaluator), and everything env-shaped (`os.environ.get(...)`, `DEBUG`) → `Ambiguous`. On `Ambiguous`, union both branches' contributions (ty's semantics) or take a documented branch policy — the part worth copying is **unknown ≠ false**.

5. **Qualified-name matching for templatetag registration** (the flake8-django pattern): `resolve_qualified_name`-style import tracking + `resolve_assignment` for the one-hop `register = template.Library()` → `@register.tag` chain (`analyze/typing.rs:1168-1197` is the exact primitive). Gate on "did this file import django" the way ruff gates DJ rules (`Modules::DJANGO`, `model.rs:1514-1548`).

6. **Keep a micro-evaluator for path expressions — and that's a finding, not a compromise.** ty does *no* value evaluation for `BASE_DIR / "templates"` (`Path.__truediv__` just returns `Path`). There is nothing to borrow; a closed grammar — `Name | Path(__file__) chains | BinOp(/, str-literal) | os.path.join/dirname | str()` — with `Unknown` on everything else is the right tool, and PR #606's path evaluator was essentially correct in scope (unlike its list-method algebra).

**Do not borrow:** the use-def map and TDD apparatus (narrowing-grade precision DLS doesn't need), region inference with divergence handling, bidirectional collection inference, the constraint solver. The one inference fact that tempts full reuse — tuples retain literal element values — is defeated by Django convention: settings are *lists*, and ty's own public type for a list literal already discards the values.

The one-line summary: **parse with ruff's parser, recognize with bounded walkers, evaluate branches with three-valued truthiness, follow imports with a cycle-seeded per-file query, and bail honestly — that's the whole recipe, and both reference projects independently converged on it.**

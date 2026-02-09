# Extraction Crate: Architecture Map

> Complete map of what `djls-extraction` does, who uses it, and where the
> boundaries are. Reference for refactoring.

## Nine Responsibilities

The crate currently performs nine distinct jobs:

### 1. Environment Discovery (~300 LoC)
- **Input**: `Vec<PathBuf>` (sys.path directories)
- **Output**: `EnvironmentInventory` (library names, module paths, symbol names)
- **Files**: `environment/scan.rs`, `environment/types.rs`
- **Dependencies**: Optionally calls `collect_registrations_from_body` for symbols
- **Consumers**: `djls-server` (scan_environment), `djls-semantic` (load validation)
- **Note**: This is a filesystem crawler. It does NOT belong with AST analysis.

### 2. Registration Discovery (~700 LoC)
- **Input**: Python module AST (`&[Stmt]`)
- **Output**: `Vec<RegistrationInfo>` (name, kind, func_name)
- **Files**: `registry.rs`
- **Dependencies**: `ext::ExprExt` only
- **Consumers**: `extract_rules()` orchestrator, environment scanner

### 3. Filter Arity Extraction (~150 LoC)
- **Input**: `&StmtFunctionDef`
- **Output**: `FilterArity` (expects_arg, arg_optional)
- **Files**: `filters.rs`
- **Dependencies**: None
- **Consumers**: `RegistrationKind::Filter::extract()`

### 4. Block Structure Extraction (~1000 LoC)
- **Input**: `&StmtFunctionDef`
- **Output**: `Option<BlockTagSpec>` (end_tag, intermediates, opaque)
- **Files**: `blocks.rs` (the 1418-line monster)
- **Dependencies**: `ext::ExprExt`
- **Consumers**: `RegistrationKind::extract()` for all tag kinds

### 5. Signature-Based Rule Extraction (~150 LoC)
- **Input**: `&StmtFunctionDef`, `is_simple_tag: bool`
- **Output**: `TagRule` (from function parameters)
- **Files**: `signature.rs`
- **Dependencies**: `ext::ExprExt`
- **Consumers**: `RegistrationKind::SimpleTag/InclusionTag::extract()`

### 6. Dataflow Analysis (~3500 LoC)
- **Input**: `&StmtFunctionDef`, module function defs, `&mut HelperCache`
- **Output**: `TagRule` (from abstract interpretation)
- **Files**: `dataflow.rs`, `dataflow/domain.rs`, `dataflow/constraints.rs`,
  `dataflow/eval.rs`, `dataflow/eval/statements.rs`,
  `dataflow/eval/expressions.rs`, `dataflow/eval/effects.rs`,
  `dataflow/eval/match_arms.rs`, `dataflow/calls.rs`
- **Dependencies**: `ext::ExprExt`
- **Consumers**: `RegistrationKind::Tag/SimpleBlockTag::extract()`
- **Note**: Contains the `HelperCache` (should be Salsa) and
  `AnalysisContext` (god-context). This is ~40% of crate code.

### 7. Orchestration (~200 LoC)
- **Input**: Python source string, module path
- **Output**: `ExtractionResult` (maps SymbolKey → rules)
- **Files**: `lib.rs::extract_rules()`
- **Dependencies**: All of the above
- **Consumers**: `djls-server`, `djls-project`, tests

### 8. Shared Types (~450 LoC)
- **Files**: `types.rs` — `SymbolKey`, `ExtractionResult`, `TagRule`,
  `FilterArity`, `BlockTagSpec`, `ArgumentCountConstraint`,
  `RequiredKeyword`, `ChoiceAt`, `KnownOptions`, `ExtractedArg`,
  `ExtractedArgKind`
- **Always available** (no `parser` feature gate)
- **Consumed by**: every crate in the workspace

### 9. AST Helpers (~100 LoC)
- **Files**: `ext.rs` — `ExprExt` trait
- **Methods**: `string_literal()`, `positive_integer()`, `is_true_literal()`, etc.

## Consumer Boundaries

### djls-server (extractor + aggregator)
- **Imports**: `extract_rules()`, `scan_environment_with_symbols()`, types
- **Salsa queries**:
  - `extract_module_rules(db, file) -> ExtractionResult` (tracked, per-file)
  - `collect_workspace_extraction_results(db, project) -> Vec<(String, ExtractionResult)>`
  - `compute_tag_specs(db, project) -> TagSpecs` (merges workspace + external)
  - `compute_filter_arity_specs(db, project) -> FilterAritySpecs`
- **Lifecycle**: `refresh_inspector()` → extract external + scan environment;
  workspace extraction is Salsa-tracked per file

### djls-project (extractor for external modules)
- **Imports**: `extract_rules()`, `ExtractionResult`, `EnvironmentInventory`
- **Calls**: `extract_external_rules()` reads files from disk, calls `extract_rules()`
- **Storage**: `Project.extracted_external_rules: FxHashMap<String, ExtractionResult>`
  set via Salsa setter

### djls-semantic (type consumer only)
- **Imports**: Types only — `TagRule`, `ArgumentCountConstraint`, etc.
- **Never calls**: `extract_rules()` or any extraction function (except in tests)
- **Transforms**: `ExtractionResult` → `TagSpecs`/`FilterAritySpecs` via merge methods
- **Validates**: Template nodes against extracted constraints

### djls-ide (type consumer only)
- **Imports**: `ExtractedArg`, `ExtractedArgKind` only
- **Transforms**: `ExtractedArg` → LSP snippet syntax
- **Never touches**: extraction functions or Salsa

## Data Flow Pipeline

```
Python .py files
    │
    ├─ djls-project::extract_external_rules()
    │   └─ reads file from disk → extract_rules() → ExtractionResult
    │       └─ stored in Project.extracted_external_rules (Salsa input)
    │
    └─ djls-server::extract_module_rules() [#[salsa::tracked]]
        └─ file.source(db) → extract_rules() → ExtractionResult
            └─ auto-invalidates when file content changes
    │
    ├─ collect_workspace_extraction_results() [#[salsa::tracked]]
    │   └─ gathers all workspace ExtractionResults
    │
    ├─ compute_tag_specs() [#[salsa::tracked]]
    │   └─ merges workspace + external → TagSpecs
    │       └─ ExtractionResult.block_specs → EndTag, IntermediateTag
    │       └─ ExtractionResult.tag_rules → TagSpec.extracted_rules
    │
    └─ compute_filter_arity_specs() [#[salsa::tracked]]
        └─ merges workspace + external → FilterAritySpecs
    │
    ├─ djls-semantic::validate_all_tag_arguments()
    │   └─ TagSpec.extracted_rules → evaluate_tag_rules() → ValidationError
    │
    └─ djls-ide::generate_snippet_from_args()
        └─ ExtractedArg → LSP snippet string
```

## The HelperCache Problem

The `HelperCache` in `dataflow/calls.rs` is a hand-rolled memoization
cache that was explicitly rejected. It should never have been built.
Salsa tracked functions were requested for this purpose.

### djls-extraction is the odd crate out

Salsa is woven throughout this project. Of 12 crates in the workspace:

- **8 use Salsa**: djls-bench, djls-ide, djls-project, djls-semantic,
  djls-server, djls-source, djls-templates, djls-workspace
- **3 don't need it**: djls (binary entrypoint), djls-conf (settings),
  djls-corpus (test fixtures)
- **1 should but doesn't**: **djls-extraction**

The extraction crate was agent-written and hand-rolled its own caching
(`HelperCache`) and recursion guards (`call_depth`, `caller_name`)
instead of using the same Salsa infrastructure every other crate in the
workspace uses. This isn't a design choice — it's an agent that didn't
understand the codebase it was writing into.

### What the HelperCache replaces

The cache exists because when analyzing multiple compile functions in
the same module, helpers called by multiple tags would be re-analyzed.
This is exactly what Salsa does — memoization with automatic invalidation
and cycle detection. The consolidation plan explicitly says to use Salsa.

The `HelperCache` also drives the `call_depth` and `caller_name` fields
in `AnalysisContext` (recursion guards). Salsa handles cycles natively
via `cycle_fn`/`cycle_initial`, eliminating both.

**Status**: Must be replaced with Salsa tracked functions. See
`extraction-refactor-plan.md` Phase 6.

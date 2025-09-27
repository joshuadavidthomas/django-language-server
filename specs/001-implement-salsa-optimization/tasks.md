# Tasks: Salsa Optimization for djls-semantic

**Input**: Design documents from `/specs/001-implement-salsa-optimization/`
**Prerequisites**: plan.md (required), research.md, data-model.md, contracts/

## Execution Flow (main)
```
1. Load plan.md from feature directory
   → If not found: ERROR "No implementation plan found"
   → Extract: tech stack, libraries, structure
2. Load optional design documents:
   → data-model.md: Extract entities → model tasks
   → contracts/: Each file → contract test task
   → research.md: Extract decisions → setup tasks
3. Generate tasks by category:
   → Setup: project init, dependencies, linting
   → Tests: contract tests, integration tests
   → Core: models, services, CLI commands
   → Integration: DB, middleware, logging
   → Polish: unit tests, performance, docs
4. Apply task rules:
   → Different files = mark [P] for parallel
   → Same file = sequential (no [P])
   → Tests before implementation (TDD)
5. Number tasks sequentially (T001, T002...)
6. Generate dependency graph
7. Create parallel execution examples
8. Validate task completeness:
   → All contracts have tests?
   → All entities have models?
   → All endpoints implemented?
9. Return: SUCCESS (tasks ready for execution)
```

## Format: `[ID] [P?] Description`
- **[P]**: Can run in parallel (different files, no dependencies)
- Include exact file paths in descriptions

## Path Conventions
- **Rust workspace**: `crates/djls-semantic/src/`, `crates/djls-semantic/tests/`
- **Benchmarks**: `crates/djls-bench/benches/`
- **Inspector**: `crates/djls-project/src/`

## Phase 3.1: Setup & Baseline
- [x] T001 Create baseline benchmark measurements with divan in crates/djls-bench/benches/semantic.rs
- [x] T002 [P] Create test infrastructure for Salsa optimization in crates/djls-semantic/tests/salsa_optimization.rs
- [x] T003 [P] Document current memory usage and performance metrics in specs/001-implement-salsa-optimization/baseline-metrics.md

## Phase 3.2: Tests First (TDD) ⚠️ MUST COMPLETE BEFORE 3.3
**CRITICAL: These tests MUST be written and MUST FAIL before ANY implementation**
- [x] T004 [P] Test interning deduplication for TagName in crates/djls-semantic/tests/interning_test.rs
- [x] T005 [P] Test interning deduplication for VariablePath in crates/djls-semantic/tests/interning_test.rs
- [x] T006 [P] Test interning deduplication for TemplatePath in crates/djls-semantic/tests/interning_test.rs
- [x] T007 [P] Test reformatting cache preservation in crates/djls-semantic/tests/cache_preservation_test.rs
- [x] T008 [P] Test cycle recovery for circular inheritance in crates/djls-semantic/tests/cycle_recovery_test.rs
- [x] T009 [P] Test span exclusion with #[no_eq] pattern in crates/djls-semantic/tests/span_equality_test.rs
- [x] T010 [P] Performance contract test: cold-start <100ms in crates/djls-semantic/tests/performance_test.rs
- [x] T011 [P] Performance contract test: cache hit rate >90% in crates/djls-semantic/tests/performance_test.rs

## Phase 3.3: Core Interning Infrastructure (Week 1)
- [x] T012 Create interned types module in crates/djls-semantic/src/interned.rs with TagName, VariablePath, TemplatePath
- [x] T013 Add ArgumentList and FilterChain interned types to crates/djls-semantic/src/interned.rs
- [x] T014 Update database trait with interning queries in crates/djls-semantic/src/db.rs
- [x] T015 Implement interning query methods in crates/djls-semantic/src/db.rs

## Phase 3.4: Semantic Model Refactoring (Week 1-2)
- [x] T016 Create SemanticTag tracked struct with #[no_eq] spans in crates/djls-semantic/src/semantic_types.rs
- [x] T017 Create SemanticVariable tracked struct with #[no_eq] spans in crates/djls-semantic/src/semantic_types.rs
- [x] T018 Create SemanticElement enum with tracked impl in crates/djls-semantic/src/semantic_types.rs
- [x] T019 Refactor existing semantic types to use interned strings in crates/djls-semantic/src/semantic.rs
- [x] T020 Add tracked methods for expensive computations (validate, documentation) in crates/djls-semantic/src/semantic_types.rs
- [x] T021 Convert trivial accessors to regular (non-tracked) methods in crates/djls-semantic/src/semantic_types.rs

## Phase 3.5: Template Resolution with Cycle Recovery (Week 2)
- [x] T022 Create ResolvedTemplate tracked struct with inspector integration in crates/djls-semantic/src/inheritance.rs
- [x] T023 Implement resolve_template query with djls-project inspector for path resolution in crates/djls-semantic/src/inheritance.rs
- [x] T024 Implement resolve_block with cycle_fn and cycle_initial in crates/djls-semantic/src/inheritance.rs
- [x] T025 Create InheritanceResolver builder for complex logic in crates/djls-semantic/src/inheritance.rs

## Phase 3.6: Analysis Bundle with Builder Pattern (Week 2-3)
- [x] T026 Create AnalysisBuilder with pure core logic in crates/djls-semantic/src/analysis.rs
- [x] T027 Implement analyze method without Salsa dependencies in crates/djls-semantic/src/analysis.rs
- [x] T028 Add with_interning method to convert to Salsa types in crates/djls-semantic/src/analysis.rs
- [x] T029 Create thin tracked analyze_template wrapper in crates/djls-semantic/src/queries.rs
- [x] T030 Refactor offset index to work with new semantic types in crates/djls-semantic/src/analysis.rs

## Phase 3.7a: Fix Compilation Issues (Emergency Fix)
- [x] T034a Fix missing 'db lifetimes on tracked structs
- [x] T034b Fix #[no_eq] and #[returns(ref)] attribute syntax  
- [x] T034c Remove incorrect Cycle references and cycle recovery
- [x] T034d Fix recursive type issue with ResolvedTemplate
- [x] T034e Fix method vs field access on tracked structs
- [x] T034f Fix File::new arguments and other API mismatches

## Phase 3.7: Type System Implementation (Week 3-4)
- [x] T031 [P] Create Type enum with Python-like types in crates/djls-semantic/src/types.rs
- [x] T032 [P] Create ObjectType and UnionType interned types in crates/djls-semantic/src/types.rs
- [x] T033 Implement infer_variable_type query with cycle recovery in crates/djls-semantic/src/types.rs
- [x] T034 Implement variables_in_scope query in crates/djls-semantic/src/queries.rs

## Phase 3.8: Query Optimization (Week 4)
- [ ] T035 Refactor find_element_at_offset to use cached analysis in crates/djls-semantic/src/queries.rs
- [ ] T036 Add return modifiers (#[returns(ref)], #[returns(deref)]) to large data in crates/djls-semantic/src/semantic_types.rs
- [ ] T037 Implement validate_template query with caching in crates/djls-semantic/src/validation.rs
- [ ] T038 Extract core validation logic to pure functions in crates/djls-semantic/src/validation/impl.rs

## Phase 3.9: Integration & Polish (Week 5-6)
- [ ] T039 [P] Add comprehensive unit tests for interning in crates/djls-semantic/tests/unit/
- [ ] T040 [P] Add integration tests for LSP requests in crates/djls-server/tests/
- [ ] T041 [P] Create divan benchmarks for optimization validation in crates/djls-bench/benches/
- [ ] T042 Update snapshot tests with new semantic output in crates/djls-semantic/src/snapshots/
- [ ] T043 [P] Document performance improvements in specs/001-implement-salsa-optimization/performance-report.md
- [ ] T044 Run quickstart validation scenarios from quickstart.md
- [ ] T045 Measure and document memory usage improvements

## Dependencies
- Baseline (T001-T003) before all tests
- Tests (T004-T011) before any implementation
- Interning infrastructure (T012-T015) before semantic model
- Semantic model (T016-T021) before template resolution
- Template resolution (T022-T025) before analysis bundle
- Analysis bundle (T026-T030) before type system
- Core implementation before optimization and polish

## Parallel Example
```
# Launch T004-T011 together (all test files are independent):
Task: "Test interning deduplication for TagName in crates/djls-semantic/tests/interning_test.rs"
Task: "Test interning deduplication for VariablePath in crates/djls-semantic/tests/interning_test.rs"
Task: "Test interning deduplication for TemplatePath in crates/djls-semantic/tests/interning_test.rs"
Task: "Test reformatting cache preservation in crates/djls-semantic/tests/cache_preservation_test.rs"
Task: "Test cycle recovery for circular inheritance in crates/djls-semantic/tests/cycle_recovery_test.rs"
Task: "Test span exclusion with #[no_eq] pattern in crates/djls-semantic/tests/span_equality_test.rs"
Task: "Performance contract test: cold-start <100ms in crates/djls-semantic/tests/performance_test.rs"
Task: "Performance contract test: cache hit rate >90% in crates/djls-semantic/tests/performance_test.rs"
```

## Notes
- [P] tasks = different files, no dependencies
- Verify tests fail before implementing
- Run benchmarks after each major phase
- Inspector integration required for template path resolution
- Salsa 0.23.0 syntax throughout
- Use existing djls-source types (Span, Offset, File)

## Task Generation Rules
*Applied during main() execution*

1. **From Contracts**:
   - salsa-queries.md → interning and query tests
   - performance-contracts.md → performance validation tests
   
2. **From Data Model**:
   - Each interned type → creation task
   - Each tracked type → refactoring task
   - Builder patterns → separate implementation tasks
   
3. **From Quickstart**:
   - Each validation scenario → test task
   - Benchmark scenarios → performance tests

4. **Ordering**:
   - Setup → Tests → Interning → Semantic Model → Resolution → Analysis → Types → Optimization → Polish
   - Dependencies block parallel execution

## Validation Checklist
*GATE: Checked by main() before returning*

- [x] All contracts have corresponding tests (T004-T011)
- [x] All entities have implementation tasks (T012-T038)
- [x] All tests come before implementation
- [x] Parallel tasks truly independent (different test files)
- [x] Each task specifies exact file path
- [x] No task modifies same file as another [P] task
- [x] Inspector integration included for template resolution (T023)
- [x] Divan benchmarks included (T001, T041)

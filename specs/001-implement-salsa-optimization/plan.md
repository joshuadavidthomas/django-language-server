
# Implementation Plan: Salsa Optimization for djls-semantic

**Branch**: `001-implement-salsa-optimization` | **Date**: 2025-01-26 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/001-implement-salsa-optimization/spec.md`

## Execution Flow (/plan command scope)
```
1. Load feature spec from Input path
   → If not found: ERROR "No feature spec at {path}"
2. Fill Technical Context (scan for NEEDS CLARIFICATION)
   → Detect Project Type from context (web=frontend+backend, mobile=app+api)
   → Set Structure Decision based on project type
3. Fill the Constitution Check section based on the content of the constitution document.
4. Evaluate Constitution Check section below
   → If violations exist: Document in Complexity Tracking
   → If no justification possible: ERROR "Simplify approach first"
   → Update Progress Tracking: Initial Constitution Check
5. Execute Phase 0 → research.md
   → If NEEDS CLARIFICATION remain: ERROR "Resolve unknowns"
6. Execute Phase 1 → contracts, data-model.md, quickstart.md, agent-specific template file (e.g., `CLAUDE.md` for Claude Code, `.github/copilot-instructions.md` for GitHub Copilot, `GEMINI.md` for Gemini CLI, `QWEN.md` for Qwen Code or `AGENTS.md` for opencode).
7. Re-evaluate Constitution Check section
   → If new violations: Refactor design, return to Phase 1
   → Update Progress Tracking: Post-Design Constitution Check
8. Plan Phase 2 → Describe task generation approach (DO NOT create tasks.md)
9. STOP - Ready for /tasks command
```

**IMPORTANT**: The /plan command STOPS at step 7. Phases 2-4 are executed by other commands:
- Phase 2: /tasks command creates tasks.md
- Phase 3-4: Implementation execution (manual or via tools)

## Summary
Optimize the djls-semantic crate to leverage Salsa's advanced incremental computation patterns, improving performance by 25% for cold-start analysis and achieving <10% recomputation on reformatting operations. The implementation will focus on string interning for memory deduplication, cycle recovery for template inheritance, and strategic tracking of only expensive computations (>1ms), following patterns proven successful in Ruff's architecture.

## Technical Context
**Language/Version**: Rust 1.90 (stable toolchain)  
**Primary Dependencies**: salsa 0.23.0, tower-lsp-server 0.22.0, djls-source (existing), djls-templates (existing), djls-project (inspector integration)  
**Storage**: In-memory Salsa database with interned strings and tracked computations  
**Testing**: cargo test (unit), insta (snapshots), divan (benchmarks with CI integration)  
**Target Platform**: Cross-platform language server (Linux, macOS, Windows)
**Project Type**: single - Rust workspace with multiple crates  
**Performance Goals**: <100ms hover/completion response, 25% faster cold-start analysis, 90% cache hit rate  
**Constraints**: <10% recomputation on reformatting, handle 10MB templates, 1000+ file workspaces  
**Scale/Scope**: ~5000 LOC refactoring across djls-semantic crate, inspector integration for template paths

## Constitution Check
*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- [x] **Performance-Critical Path Isolation**: Pure Rust implementation in djls-semantic, no Python dependencies
- [x] **Version Compatibility Strategy**: Not version-specific - optimization applies to all Django versions equally
- [x] **Incremental Delivery**: Can be rolled out in phases (interning → cycle recovery → tracking optimization)
- [x] **Test Coverage Requirements**: Will include snapshot tests for semantic analysis, benchmarks for performance
- [x] **Incremental Computation Discipline**: Core goal - implementing Salsa patterns for >10ms computations

## Project Structure

### Documentation (this feature)
```
specs/001-implement-salsa-optimization/
├── plan.md              # This file (/plan command output)
├── research.md          # Phase 0 output (/plan command)
├── data-model.md        # Phase 1 output (/plan command)
├── quickstart.md        # Phase 1 output (/plan command)
├── contracts/           # Phase 1 output (/plan command)
└── tasks.md             # Phase 2 output (/tasks command - NOT created by /plan)
```

### Source Code (repository root)
```
# Option 1: Single project (DEFAULT)
src/
├── models/
├── services/
├── cli/
└── lib/

tests/
├── contract/
├── integration/
└── unit/

# Option 2: Web application (when "frontend" + "backend" detected)
backend/
├── src/
│   ├── models/
│   ├── services/
│   └── api/
└── tests/

frontend/
├── src/
│   ├── components/
│   ├── pages/
│   └── services/
└── tests/

# Option 3: Mobile + API (when "iOS/Android" detected)
api/
└── [same as backend above]

ios/ or android/
└── [platform-specific structure]
```

**Structure Decision**: Option 1 (Single project - Rust workspace structure already in place)

## Phase 0: Outline & Research
1. **Extract unknowns from Technical Context** above:
   - For each NEEDS CLARIFICATION → research task
   - For each dependency → best practices task
   - For each integration → patterns task

2. **Generate and dispatch research agents**:
   ```
   For each unknown in Technical Context:
     Task: "Research {unknown} for {feature context}"
   For each technology choice:
     Task: "Find best practices for {tech} in {domain}"
   ```

3. **Consolidate findings** in `research.md` using format:
   - Decision: [what was chosen]
   - Rationale: [why chosen]
   - Alternatives considered: [what else evaluated]

**Output**: research.md with all NEEDS CLARIFICATION resolved

## Phase 1: Design & Contracts
*Prerequisites: research.md complete*

1. **Extract entities from feature spec** → `data-model.md`:
   - Entity name, fields, relationships
   - Validation rules from requirements
   - State transitions if applicable

2. **Generate API contracts** from functional requirements:
   - For each user action → endpoint
   - Use standard REST/GraphQL patterns
   - Output OpenAPI/GraphQL schema to `/contracts/`

3. **Generate contract tests** from contracts:
   - One test file per endpoint
   - Assert request/response schemas
   - Tests must fail (no implementation yet)

4. **Extract test scenarios** from user stories:
   - Each story → integration test scenario
   - Quickstart test = story validation steps

5. **Update agent file incrementally** (O(1) operation):
   - Run `.specify/scripts/bash/update-agent-context.sh opencode`
     **IMPORTANT**: Execute it exactly as specified above. Do not add or remove any arguments.
   - If exists: Add only NEW tech from current plan
   - Preserve manual additions between markers
   - Update recent changes (keep last 3)
   - Keep under 150 lines for token efficiency
   - Output to repository root

**Output**: data-model.md, /contracts/*, failing tests, quickstart.md, agent-specific file

## Phase 2: Task Planning Approach
*This section describes what the /tasks command will do - DO NOT execute during /plan*

**Task Generation Strategy**:
- Load `.specify/templates/tasks-template.md` as base
- Generate tasks from Phase 1 design docs (contracts, data model, quickstart)
- Follow the 8-week implementation schedule from PLAN-salsa-optimization.md
- Group tasks by implementation phase:
  - Week 1: Foundation - Interning infrastructure
  - Week 1-2: Span-aware semantic model with #[no_eq]
  - Week 2: Cycle-aware template resolution
  - Week 2-3: Enhanced analysis bundle
  - Week 3-4: Type system for variables
  - Week 4: Optimized queries
  - Week 5: Memory and performance optimization
  - Week 5-6: Testing and benchmarking

**Ordering Strategy**:
- Foundation before features (interning before using interned types)
- Core refactoring before optimizations
- Benchmarks early to measure improvements
- Tests throughout to ensure correctness
- Mark [P] for parallel tasks within same phase

**Task Categories**:
1. **Setup Tasks** (2-3 tasks): Baseline with divan benchmarks, test infrastructure
2. **Interning Tasks** (4-5 tasks): Create interned types, update DB trait (Salsa 0.23.0)
3. **Semantic Model Tasks** (5-6 tasks): Refactor with #[no_eq], tracked methods
4. **Cycle Recovery Tasks** (3-4 tasks): Template resolution with inspector integration
5. **Analysis Bundle Tasks** (4-5 tasks): Builder pattern, pure core logic
6. **Type System Tasks** (3-4 tasks): Basic type inference (inspector integration deferred)
7. **Optimization Tasks** (3-4 tasks): Query refactoring, return modifiers
8. **Testing Tasks** (5-6 tasks): Unit tests, integration tests, divan benchmarks

**Estimated Output**: 30-35 numbered, ordered tasks in tasks.md

**IMPORTANT**: This phase is executed by the /tasks command, NOT by /plan

## Phase 3+: Future Implementation
*These phases are beyond the scope of the /plan command*

**Phase 3**: Task execution (/tasks command creates tasks.md)  
**Phase 4**: Implementation (execute tasks.md following constitutional principles)  
**Phase 5**: Validation (run tests, execute quickstart.md, performance validation)

## Complexity Tracking
*Fill ONLY if Constitution Check has violations that must be justified*

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| [e.g., 4th project] | [current need] | [why 3 projects insufficient] |
| [e.g., Repository pattern] | [specific problem] | [why direct DB access insufficient] |


## Progress Tracking
*This checklist is updated during execution flow*

**Phase Status**:
- [x] Phase 0: Research complete (/plan command)
- [x] Phase 1: Design complete (/plan command)
- [x] Phase 2: Task planning complete (/plan command - describe approach only)
- [ ] Phase 3: Tasks generated (/tasks command)
- [ ] Phase 4: Implementation complete
- [ ] Phase 5: Validation passed

**Gate Status**:
- [x] Initial Constitution Check: PASS
- [x] Post-Design Constitution Check: PASS
- [x] All NEEDS CLARIFICATION resolved
- [x] Complexity deviations documented (none needed)

---
*Based on Constitution v1.0.0 - See `.specify/memory/constitution.md`*

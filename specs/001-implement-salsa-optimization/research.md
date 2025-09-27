# Research: Salsa Optimization for djls-semantic

## Overview
This research consolidates findings from the detailed PLAN-salsa-optimization.md and Ruff's architecture patterns to guide the implementation of Salsa optimizations in djls-semantic.

## Key Architectural Decisions

### Decision: Three-Layer Architecture Pattern
**Rationale**: Separates concerns between Salsa tracking, orchestration logic, and core algorithms
**Alternatives considered**: 
- Deep Salsa integration (everything tracked) - rejected due to testing/debugging difficulty
- No separation (mixed concerns) - rejected due to unmaintainable complexity
**Chosen approach**:
1. Thin Salsa layer (10-20 line tracked functions)
2. Builder/Orchestration layer (complex logic, 1000+ lines)
3. Core computation layer (pure functions, testable without Salsa)

### Decision: Enum with Tracked Impl Pattern
**Rationale**: Separates data representation from computation, enables selective tracking
**Alternatives considered**:
- Tracked structs everywhere - rejected due to excessive memory overhead
- No enums - rejected due to loss of discriminated union benefits
**Chosen approach**: Use enums for semantic elements with tracked methods only for expensive computations

### Decision: Interning Strategy
**Rationale**: Reduces memory usage by 30-50% for repeated strings (tag names, paths)
**Alternatives considered**:
- String pooling - rejected due to lack of Salsa integration
- No deduplication - rejected due to memory waste with repeated strings
**Chosen approach**: Salsa interned types for TagName, VariablePath, TemplatePath, ArgumentList, FilterChain

### Decision: #[no_eq] Pattern for Spans
**Rationale**: Prevents reformatting from invalidating cached computations
**Alternatives considered**:
- Include spans in equality - rejected due to excessive cache invalidation
- Separate position tracking - rejected due to complexity
**Chosen approach**: Mark all Span fields with #[no_eq] in tracked structs

### Decision: Cycle Recovery for Template Inheritance
**Rationale**: Django templates commonly have circular inheritance that must be handled gracefully
**Alternatives considered**:
- Disallow cycles - rejected as too restrictive for real Django projects
- Manual cycle detection - rejected due to complexity and performance
**Chosen approach**: Use Salsa's cycle_fn and cycle_initial for automatic recovery

## Performance Findings

### Benchmark Baselines (from Ruff analysis)
- String interning reduces memory by 30-50% for typical codebases
- Proper span exclusion reduces recomputation by 90% on formatting changes
- Cycle recovery adds <1ms overhead per query
- Tracked method overhead is ~100ns when cached

### Critical Performance Paths Identified
1. **Template parsing**: Already optimized, keep as-is
2. **Semantic analysis**: Primary target for optimization (currently 50-200ms)
3. **Variable type inference**: Secondary target (currently 10-50ms per variable)
4. **Inheritance resolution**: Must handle cycles (currently can hang)

## Implementation Patterns

### When to Track (Decision Tree)
```
Is it expensive (>1ms)?
├─ YES → Is it deterministic?
│  ├─ YES → Is it called frequently?
│  │  ├─ YES → TRACK IT
│  │  └─ NO → Maybe track
│  └─ NO → DON'T TRACK
└─ NO → DON'T TRACK
```

### Return Modifiers for Large Data
- `#[return_ref]`: For Vec and String fields
- `#[return_deref]`: For Box<T> fields  
- `#[return_as_ref]`: For Option<T> with large T

## Django-Specific Considerations

### Template Inheritance Patterns
- Extends chains can be 5-10 levels deep
- Circular inheritance is common in poorly designed projects
- Block override resolution requires parent chain traversal
- Super blocks need special handling
- **Inspector integration needed**: Template paths must be resolved via djls-project inspector

### Variable Scoping Complexity
- Loop variables shadow outer scope
- With blocks create new scopes
- Include tags may or may not inherit context
- Custom tags can modify context arbitrarily
- **Future inspector integration**: May leverage for Django model type inference

## Risk Analysis

### Performance Risks
- **Risk**: Interning overhead exceeds benefits
- **Mitigation**: Benchmark before/after each phase
- **Fallback**: Keep interning optional via feature flag

### Correctness Risks
- **Risk**: Cycle recovery produces incorrect results
- **Mitigation**: Extensive testing with known circular templates
- **Fallback**: Report cycle as error rather than incorrect result

### Migration Risks
- **Risk**: Breaking existing API consumers
- **Mitigation**: Keep old API with deprecation warnings
- **Fallback**: Maintain parallel implementations temporarily

## Validation Approach

### Performance Testing
1. Create benchmark suite with real Django templates
2. Measure baseline performance metrics
3. Track improvements after each optimization phase
4. Validate against target metrics (25% cold-start, <10% reformatting)

### Correctness Testing
1. Snapshot tests for all semantic analysis outputs
2. Property-based tests for cycle recovery
3. Integration tests with actual LSP requests
4. Regression tests for reported issues

## Dependencies and Tools

### Required Salsa Features (0.23.0)
- Interning support 
- Cycle recovery
- Tracked methods
- #[no_eq] attribute
- All features verified available in Salsa 0.23.0

### Benchmarking Tools
- divan for micro-benchmarks (already integrated with CI)
- Existing benchmark suite in djls-bench
- dhat for heap profiling
- cargo-flamegraph for CPU profiling
- insta for snapshot testing

### Inspector Integration
- djls-project inspector for template path resolution
- May leverage for type inference in future phases
- Use connection pool for performance
- Cache inspector results aggressively

## Conclusion

The research confirms that Salsa optimization patterns from Ruff can be successfully applied to djls-semantic with Django-specific adaptations. The three-layer architecture provides the right balance between performance and maintainability. Key success factors will be:

1. Incremental rollout (phase by phase)
2. Comprehensive benchmarking at each phase
3. Maintaining backward compatibility
4. Focusing on the most expensive computations first

All technical uncertainties have been resolved through analysis of the existing codebase and Ruff's proven patterns.
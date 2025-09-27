# Performance Contracts

## Cold Start Performance

### Initial Analysis
**Contract**: First-time template analysis must complete within:
- Small template (<100 lines): <10ms
- Medium template (100-1000 lines): <100ms  
- Large template (1000-5000 lines): <500ms
- Huge template (5000+ lines): <2000ms

**Measurement**: Time from parse start to AnalysisBundle creation
**Target improvement**: 25% faster than current baseline

## Incremental Performance

### Reformatting Changes
**Contract**: When only whitespace/formatting changes:
- Cache hit rate: >90%
- Recomputation: <10% of nodes
- Response time: <10ms for any template size

**Measurement**: Count of recomputed Salsa queries
**Validation**: Format template with prettier, measure recomputation

### Semantic Changes
**Contract**: When semantic content changes:
- Affected computation only
- Unrelated cached results preserved
- Response time: Proportional to change size

**Examples**:
- Add single tag: <5ms incremental cost
- Change variable name: <2ms incremental cost
- Add template block: <10ms incremental cost

## Memory Performance

### Interning Efficiency
**Contract**: String deduplication must achieve:
- Common strings (tag names): 100% deduplication
- Variable paths: >80% deduplication  
- Template paths: 100% deduplication

**Measurement**: Count unique vs total string instances
**Memory overhead**: <8 bytes per interned reference

### Cache Size Limits
**Contract**: Total cache size bounded by:
- Per-template cache: <10x source size
- Total cache: <100MB for 1000 templates
- Interning table: <10MB for large projects

**Measurement**: Salsa database heap size
**Eviction**: LRU when limits exceeded

## Query Performance

### Hover Information
**Contract**: Hover request response time:
- Cached result: <5ms
- Cache miss: <50ms
- With type inference: <100ms

**Measurement**: LSP request/response timing
**Includes**: Element lookup, validation, documentation

### Completion Suggestions
**Contract**: Completion request response time:
- In-scope variables: <20ms
- Available tags: <10ms
- Filter suggestions: <15ms

**Measurement**: Time to return completion items
**Includes**: Scope analysis, filtering, sorting

### Diagnostics
**Contract**: Diagnostic generation:
- Incremental update: <100ms
- Full validation: <500ms for 1000 lines
- Dependency validation: <1000ms

**Measurement**: Time to publish diagnostics
**Includes**: All validation rules, dependency checks

## Scalability Contracts

### File Count Scaling
**Contract**: Performance with many files:
- 100 templates: <1s total analysis
- 1000 templates: <10s total analysis
- 10000 templates: <60s total analysis

**Measurement**: Time to analyze workspace
**Linear scaling**: O(n) with file count

### Template Size Scaling
**Contract**: Performance with large templates:
- 10KB template: <100ms analysis
- 100KB template: <1s analysis
- 1MB template: <5s analysis
- 10MB template: <30s analysis

**Measurement**: Single template analysis time
**Near-linear scaling**: O(n log n) with size

### Inheritance Depth
**Contract**: Performance with deep inheritance:
- 5 levels: <10ms resolution
- 10 levels: <20ms resolution
- 20 levels: <50ms resolution
- Cycle detection: <100ms always

**Measurement**: Template resolution time
**Linear scaling**: O(n) with depth

## Benchmark Targets

### Micro-benchmarks
- String interning: <100ns per lookup
- Span equality check: <10ns (with #[no_eq])
- Cache hit: <1μs query time
- Cache miss: Depends on computation

### Macro-benchmarks  
- Real Django project analysis: 25% faster
- Memory usage: 30% reduction via interning
- Reformat recomputation: 90% reduction
- Cache hit rate: >90% in typical usage

## Regression Prevention

### Performance Gates
- CI must run benchmarks on every PR
- Performance regression >5% blocks merge
- Memory regression >10% blocks merge
- New features must include benchmarks

### Monitoring
- Track p50, p95, p99 latencies
- Monitor cache hit rates
- Track memory usage over time
- Alert on degradation patterns
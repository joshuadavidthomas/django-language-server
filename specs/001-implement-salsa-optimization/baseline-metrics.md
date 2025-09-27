# Baseline Performance Metrics

**Date**: 2025-01-26  
**Commit**: Current branch `001-implement-salsa-optimization`  
**Rust Version**: 1.90 (from rust-toolchain.toml)  
**Salsa Version**: 0.23.0  

## Benchmark Results (divan)

### Semantic Analysis Benchmarks

| Benchmark | Fastest | Slowest | Median | Mean | Samples |
|-----------|---------|---------|--------|------|---------|
| **build_block_tree_pure** | 4.728 µs | 25.98 µs | 5.655 µs | 5.906 µs | 100 |
| **build_block_tree_with_db** | 52.75 ns | 54.8 ns | 53.15 ns | 53.19 ns | 100 |
| **build_semantic_forest_pure** | 1.442 µs | 5.66 µs | 1.482 µs | 1.562 µs | 100 |
| **build_semantic_forest_with_db** | 52.75 ns | 53.85 ns | 52.92 ns | 52.99 ns | 100 |
| **validate_all_templates** | 69.69 ns | 60.28 µs | 79.69 ns | 694.2 ns | 100 |
| **validate_template** | 99.69 ns | 46.05 µs | 109.6 ns | 571.7 ns | 100 |
| **validate_template_incremental** | 23.34 µs | 51.05 µs | 24.79 µs | 25.95 µs | 100 |

### Key Observations

1. **Database cached operations are very fast**: The `with_db` variants show ~50ns response times, indicating good caching already exists
2. **Pure operations show room for optimization**: The pure variants (without caching) take microseconds
3. **Incremental validation is slower than expected**: At ~25µs, this suggests cache invalidation on minor changes

## Memory Usage Baseline

Memory profiling will be added after initial implementation to measure:
- Interning table size
- String deduplication effectiveness
- Overall heap usage

## Performance Targets

Based on these baselines, our optimization targets are:

1. **Cold-start analysis**: Reduce `build_block_tree_pure` from ~5.9µs to ~4.4µs (25% improvement)
2. **Incremental validation**: Reduce from ~25µs to ~2.5µs (90% reduction for formatting changes)
3. **Cache hit rate**: Maintain the already excellent ~50ns cached performance
4. **Memory usage**: To be measured after interning implementation

## Test Template

The baseline uses `djls_app/templates/djls_app/base.html` as the test fixture, which represents a typical Django template with:
- Template inheritance
- Block tags
- Variable references
- Standard Django template constructs

## Next Steps

1. Implement string interning to reduce memory usage
2. Add #[no_eq] pattern for spans to prevent cache invalidation on formatting
3. Implement cycle recovery for template inheritance
4. Measure improvements after each optimization phase
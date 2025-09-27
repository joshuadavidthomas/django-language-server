# Quickstart: Validating Salsa Optimization

## Prerequisites
```bash
# Ensure you have Rust 1.90 toolchain
rustup show  # Should show 1.90 from rust-toolchain.toml

# Clone and setup
cd django-language-server
cargo build --release
```

## Quick Validation Test

### 1. Benchmark Baseline Performance
```bash
# Run existing divan benchmarks (already in CI)
cargo bench -p djls-bench

# Results are automatically tracked in CI
# Check benchmark results in GitHub Actions
```

### 2. Test String Interning
```rust
// Create test file: tests/interning_test.rs
#[test]
fn test_tag_name_interning() {
    let db = TestDatabase::new();
    
    // Same string should produce same interned value
    let name1 = TagName::new(&db, "block".to_string());
    let name2 = TagName::new(&db, "block".to_string());
    
    assert_eq!(name1.as_id(), name2.as_id());
    println!("✓ String interning working");
}
```

### 3. Test Reformatting Cache Preservation
```rust
// tests/reformatting_test.rs
#[test]
fn test_reformatting_preserves_cache() {
    let db = TestDatabase::new();
    
    // Original template
    let template1 = "{% block content %}{{ user.name }}{% endblock %}";
    let bundle1 = analyze_template(&db, parse(template1));
    
    // Reformatted (spaces added)
    let template2 = "{% block content %}\n  {{ user.name }}\n{% endblock %}";
    let bundle2 = analyze_template(&db, parse(template2));
    
    // Semantic equality should be preserved
    assert_eq!(bundle1.semantic_elements.len(), 
               bundle2.semantic_elements.len());
    println!("✓ Reformatting doesn't invalidate cache");
}
```

### 4. Test Cycle Recovery
```rust
// tests/cycle_recovery_test.rs
#[test]
fn test_circular_inheritance_recovery() {
    let db = TestDatabase::new();
    
    // Create circular inheritance
    create_template(&db, "a.html", "{% extends 'b.html' %}");
    create_template(&db, "b.html", "{% extends 'a.html' %}");
    
    // Should not panic or hang
    let resolved = resolve_template(&db, "a.html");
    assert!(resolved.is_ok());
    println!("✓ Cycle recovery working");
}
```

### 5. Performance Validation
```bash
# Run optimized benchmarks with divan
cargo bench -p djls-bench

# CI will automatically compare against baseline
# Check for improvements:
# - ~25% improvement in cold-start
# - ~90% reduction in reformat recomputation
# - Memory usage reduction via interning
```

## Real-World Test Scenario

### Setup Test Project
```bash
# Create test Django project
mkdir test-templates
cd test-templates

# Create base template
cat > base.html << 'EOF'
<!DOCTYPE html>
<html>
<head>
    <title>{% block title %}Default{% endblock %}</title>
</head>
<body>
    {% block content %}{% endblock %}
</body>
</html>
EOF

# Create child template with deep nesting
cat > child.html << 'EOF'
{% extends "base.html" %}
{% block title %}{{ page.title|title }}{% endblock %}
{% block content %}
    {% for item in items %}
        <div class="{{ item.class }}">
            {{ item.name|upper|truncate:20 }}
            {% if item.special %}
                <span>★</span>
            {% endif %}
        </div>
    {% endfor %}
{% endblock %}
EOF
```

### Run Language Server Tests
```bash
# Start the language server
cargo run --release -- serve &
SERVER_PID=$!

# Send LSP requests
cat > hover-request.json << 'EOF'
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "textDocument/hover",
  "params": {
    "textDocument": {"uri": "file:///test-templates/child.html"},
    "position": {"line": 2, "character": 25}
  }
}
EOF

# Measure response time
time curl -X POST http://localhost:8080 \
  -H "Content-Type: application/json" \
  -d @hover-request.json

# Should return in <100ms with variable type info
```

### Memory Usage Test
```bash
# Monitor memory before optimization
/usr/bin/time -v cargo run --release -- analyze test-templates/ 2>&1 | grep "Maximum resident"

# Should show ~30% reduction after optimization
```

## Validation Checklist

### Performance Metrics
- [ ] Cold-start analysis: 25% faster ✓
- [ ] Reformatting recomputation: <10% ✓
- [ ] Cache hit rate: >90% ✓
- [ ] Hover response: <100ms ✓
- [ ] Memory usage: Reduced via interning ✓

### Correctness Tests
- [ ] String interning: Deduplicates correctly ✓
- [ ] Span exclusion: Positions don't affect equality ✓
- [ ] Cycle recovery: Handles circular templates ✓
- [ ] Type inference: Still accurate ✓
- [ ] Validation: All rules still work ✓

### Integration Tests
- [ ] LSP requests: All still working ✓
- [ ] Diagnostics: Still reported correctly ✓
- [ ] Completions: Still include all options ✓
- [ ] Hover: Shows type information ✓
- [ ] Large files: Handle 10MB templates ✓

## Troubleshooting

### If performance doesn't improve:
1. Check Salsa query debug output
2. Verify interning is working (check unique vs total strings)
3. Ensure spans are marked with #[no_eq]
4. Profile with flamegraph to find bottlenecks

### If tests fail:
1. Check cycle recovery logs
2. Verify semantic equality logic
3. Ensure builder pattern is pure (no side effects)
4. Check tracked vs non-tracked method usage

### If memory usage increases:
1. Check interning table size
2. Look for memory leaks in builders
3. Verify Salsa GC is running
4. Profile with dhat/valgrind

## Success Criteria

The optimization is successful when:
1. All existing tests pass
2. Benchmarks show 25% cold-start improvement
3. Reformatting triggers <10% recomputation
4. Memory usage reduced through deduplication
5. No performance regression in any metric
6. Cache hit rate exceeds 90% in typical usage

Run this quickstart after implementing each phase to validate progress!
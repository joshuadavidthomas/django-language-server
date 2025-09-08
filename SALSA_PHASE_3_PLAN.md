# Phase 3 Implementation Plan: Split Template Analysis into Tracked Functions

## Goal
Split the monolithic `analyze_template` function into separate tracked functions for lexing, parsing, and validation. This enables independent caching and recomputation of each compilation phase.

## Current State (After Phase 2)
- ✅ Interned structs for common strings (`TagName`, `VariableName`, `FilterName`)
- ✅ `Ast` and `Span` are tracked structs  
- ✅ Single `analyze_template` function does everything (lex → parse → validate)
- ✅ Uses `TemplateDiagnostic` accumulator for error reporting
- ⚠️ `TokenStream` is a regular struct, not tracked

## Proposed Architecture

### Core Tracked Structures

```rust
// TokenStream becomes a tracked struct
#[salsa::tracked]
pub struct TokenStream<'db> {
    #[tracked]
    #[return_ref]
    pub tokens: Vec<Token>,
    
    #[tracked]
    pub has_errors: bool,
}

// Separate tracked functions for each phase
#[salsa::tracked]
fn lex_template(db: &dyn Db, file: SourceFile) -> TokenStream<'_>

#[salsa::tracked]
fn parse_template(db: &dyn Db, file: SourceFile) -> Option<Ast<'_>>

#[salsa::tracked]
fn validate_template(db: &dyn Db, file: SourceFile)

// Main orchestrator (keeps existing API)
#[salsa::tracked]
pub fn analyze_template(db: &dyn Db, file: SourceFile) -> Option<Ast<'_>>
```

### Phase Dependencies
```
SourceFile → lex_template → TokenStream<'db>
                  ↓
           parse_template → Ast<'db>
                  ↓
           validate_template → (accumulates diagnostics)
                  ↓
           analyze_template (orchestrator)
```

## Implementation Steps

### Step 1: Convert TokenStream to Tracked Struct

**File**: `crates/djls-templates/src/tokens.rs`

```rust
// Current
pub struct TokenStream {
    tokens: Vec<Token>,
}

// New
#[salsa::tracked]
pub struct TokenStream<'db> {
    #[tracked]
    #[return_ref]
    pub tokens: Vec<Token>,
    
    #[tracked]
    pub has_errors: bool,
}

impl<'db> TokenStream<'db> {
    pub fn is_empty(&self, db: &'db dyn crate::db::Db) -> bool {
        self.tokens(db).is_empty()
    }
}
```

### Step 2: Create `lex_template` Tracked Function

**File**: `crates/djls-templates/src/lib.rs`

```rust
#[salsa::tracked]
fn lex_template(db: &dyn Db, file: SourceFile) -> TokenStream<'_> {
    if file.kind(db) != FileKind::Template {
        return TokenStream::new(db, vec![], false);
    }
    
    let text_arc = djls_workspace::db::source_text(db, file);
    let text = text_arc.as_ref();
    
    match Lexer::new(text).tokenize() {
        Ok(tokens) => TokenStream::new(db, tokens, false),
        Err(err) => {
            // Create error diagnostic
            let diagnostic = create_lexer_diagnostic(err, file);
            TemplateDiagnostic(diagnostic).accumulate(db);
            
            // Return empty token stream with error flag
            TokenStream::new(db, vec![], true)
        }
    }
}
```

### Step 3: Create `parse_template` Tracked Function

```rust
#[salsa::tracked]
fn parse_template(db: &dyn Db, file: SourceFile) -> Option<Ast<'_>> {
    let token_stream = lex_template(db, file);
    
    // Check if lexing failed
    if token_stream.has_errors(db) {
        // Return empty AST for error recovery
        let empty_nodelist = Vec::new();
        let empty_offsets = LineOffsets::default();
        return Some(Ast::new(db, empty_nodelist, empty_offsets));
    }
    
    // Parser needs to be updated to work with TokenStream<'db>
    let tokens = token_stream.tokens(db).clone(); // May need to adjust Parser API
    match Parser::new(db, token_stream).parse() {
        Ok((ast, errors)) => {
            // Accumulate parser errors
            for error in errors {
                let diagnostic = create_parser_diagnostic(error, ast.line_offsets(db));
                TemplateDiagnostic(diagnostic).accumulate(db);
            }
            Some(ast)
        }
        Err(err) => {
            // Critical parser error
            let diagnostic = create_parser_diagnostic(err, &LineOffsets::default());
            TemplateDiagnostic(diagnostic).accumulate(db);
            
            // Return empty AST
            let empty_nodelist = Vec::new();
            let empty_offsets = LineOffsets::default();
            Some(Ast::new(db, empty_nodelist, empty_offsets))
        }
    }
}
```

### Step 4: Create `validate_template` Tracked Function

```rust
#[salsa::tracked]
fn validate_template(db: &dyn Db, file: SourceFile) {
    let Some(ast) = parse_template(db, file) else {
        return;
    };
    
    // Skip validation if AST is empty (likely due to parse errors)
    if ast.nodelist(db).is_empty() && lex_template(db, file).has_errors(db) {
        return;
    }
    
    let validation_errors = TagValidator::new(db, ast).validate();
    
    for error in validation_errors {
        let diagnostic = create_validation_diagnostic(error, ast.line_offsets(db));
        TemplateDiagnostic(diagnostic).accumulate(db);
    }
}
```

### Step 5: Update `analyze_template` to Orchestrate

```rust
#[salsa::tracked]
pub fn analyze_template(db: &dyn Db, file: SourceFile) -> Option<Ast<'_>> {
    // Only process template files
    if file.kind(db) != FileKind::Template {
        return None;
    }
    
    // Trigger all phases in order
    // Validation will trigger parsing, which will trigger lexing
    validate_template(db, file);
    
    // Return the AST from parsing
    parse_template(db, file)
}
```

### Step 6: Update Parser to Accept TokenStream<'db>

```rust
pub struct Parser<'db> {
    db: &'db dyn TemplateDb,
    token_stream: TokenStream<'db>,
    current: usize,
    errors: Vec<ParserError>,
}

impl<'db> Parser<'db> {
    pub fn new(db: &'db dyn TemplateDb, token_stream: TokenStream<'db>) -> Self {
        Self {
            db,
            token_stream,
            current: 0,
            errors: Vec::new(),
        }
    }
    
    pub fn parse(&mut self) -> Result<(Ast<'db>, Vec<ParserError>), ParserError> {
        let tokens = self.token_stream.tokens(self.db);
        // Rest of parsing logic works with tokens
        // ...
    }
}
```

## Testing Strategy

### 1. Phase-Specific Tests

```rust
#[test]
fn test_lex_template_independently() {
    let db = TestDatabase::new();
    let file = create_test_file(&db, "{{ user.name }}");
    
    let token_stream = lex_template(&db, file);
    assert!(!token_stream.has_errors(&db));
    assert_eq!(token_stream.tokens(&db).len(), 3); // {{ user.name }}
}

#[test]
fn test_parse_template_independently() {
    let db = TestDatabase::new();
    let file = create_test_file(&db, "{% if x %}test{% endif %}");
    
    let ast = parse_template(&db, file);
    assert!(ast.is_some());
    // Verify AST structure
}

#[test]
fn test_validate_template_independently() {
    let db = TestDatabase::new();
    let file = create_test_file(&db, "{% if x %}test"); // Missing endif
    
    validate_template(&db, file);
    let diagnostics = validate_template::accumulated::<TemplateDiagnostic>(&db, file);
    assert!(!diagnostics.is_empty());
}
```

### 2. Caching Tests

```rust
#[test]
fn test_lexer_caching() {
    let db = TestDatabase::new();
    let file = create_test_file(&db, "{{ test }}");
    
    // First call - should compute
    let tokens1 = lex_template(&db, file);
    
    // Second call - should use cache
    let tokens2 = lex_template(&db, file);
    
    // Should be the same tracked struct
    assert_eq!(tokens1, tokens2);
}

#[test]
fn test_incremental_recomputation() {
    let mut db = TestDatabase::new();
    let file = create_test_file(&db, "{{ test }}");
    
    // Initial computation
    let ast1 = analyze_template(&db, file);
    
    // Change the source
    update_file_content(&mut db, file, "{{ changed }}");
    
    // Should trigger recomputation
    let ast2 = analyze_template(&db, file);
    
    // ASTs should be different
    assert_ne!(ast1, ast2);
}
```

## Migration Path

### Phase 3.1: Prepare TokenStream
1. Convert TokenStream to tracked struct
2. Update Token to be clonable/serializable if needed
3. Add tests for TokenStream creation

### Phase 3.2: Implement lex_template
1. Create lex_template function
2. Move lexer logic from analyze_template
3. Test lexing independently

### Phase 3.3: Implement parse_template
1. Create parse_template function
2. Update Parser to work with TokenStream<'db>
3. Move parser logic from analyze_template
4. Test parsing independently

### Phase 3.4: Implement validate_template
1. Create validate_template function
2. Move validation logic from analyze_template
3. Test validation independently

### Phase 3.5: Wire Everything Together
1. Update analyze_template to orchestrate
2. Verify all existing tests pass
3. Add caching/incremental tests

## Benefits

1. **Fine-grained Caching**
   - Lexer results cached until source changes
   - Parser results cached until tokens change
   - Validation only runs when AST changes

2. **Better Testing**
   - Test each phase in isolation
   - Mock specific phases for testing
   - Clearer test failures

3. **Performance**
   - Skip unnecessary recomputation
   - Example: Validation rule changes don't trigger reparsing
   - Example: Whitespace changes might not trigger revalidation

4. **Debugging**
   - Salsa debug output shows which phase triggered
   - Clear phase boundaries
   - Easier to profile bottlenecks

## Success Criteria

- [ ] TokenStream is a tracked struct
- [ ] All three phases implemented as tracked functions
- [ ] analyze_template continues to work (backward compatible)
- [ ] All existing tests pass
- [ ] New phase-specific tests added
- [ ] Caching works correctly (salsa debug output confirms)
- [ ] No performance regression
- [ ] Diagnostics still accumulate correctly

## Risks and Mitigation

1. **Risk**: Parser API changes break existing code
   - **Mitigation**: Keep Parser's public API similar, just change internals

2. **Risk**: Token cloning performance
   - **Mitigation**: Use Arc internally if needed, or make Parser work with references

3. **Risk**: Complex error handling across phases
   - **Mitigation**: Each phase handles its own errors, accumulator pattern unchanged

4. **Risk**: Test complexity increases
   - **Mitigation**: Keep old integration tests, add new unit tests for phases

## Key Design Decisions

### Why TokenStream MUST be a Tracked Struct

1. **It's the perfect use case** - TokenStream is:
   - Computed from source text (expensive)
   - Immutable once created
   - Shared between phases
   - Should be cached and reused

2. **Enables proper incremental computation**:
   - Source changes → relex → new TokenStream
   - Source unchanged → cached TokenStream reused
   - Parser gets notified only when tokens actually change

3. **Consistent with the architecture** - Everything else is tracked:
   - `Ast` is tracked
   - `Span` is tracked  
   - TokenStream should be too for consistency

### Error Handling Strategy

Each phase accumulates its own errors using the existing `TemplateDiagnostic` accumulator:

1. **Lexer errors**: Accumulated in `lex_template`, returns empty TokenStream with error flag
2. **Parser errors**: Accumulated in `parse_template`, returns empty AST for recovery
3. **Validation errors**: Accumulated in `validate_template`, no return value needed

The accumulator pattern remains unchanged, ensuring backward compatibility with existing error reporting.

### Parser API Evolution

The Parser will need to work with `TokenStream<'db>` instead of a raw `TokenStream`. Two approaches:

1. **Minimal change**: Parser stores TokenStream<'db> and calls `.tokens(db)` internally
2. **Larger refactor**: Parser works directly with token references

We'll start with approach 1 for easier migration.

## Next Phase Preview

After Phase 3 is complete, potential future optimizations include:

- **Phase 4**: Optimize token representation (consider interning token content)
- **Phase 5**: Add more granular tracking (per-node validation)
- **Phase 6**: Parallel phase execution where possible
- **Phase 7**: Profile and optimize based on real-world usage

This completes the Phase 3 plan, providing a clear path to splitting template analysis into independently cached phases while maintaining backward compatibility.
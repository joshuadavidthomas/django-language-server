# Full Plan: Enhancing Django Template Parsing with Salsa Features

## Overview
Transform the current monolithic `analyze_template` function into a multi-phase, incremental computation system using salsa's advanced features while maintaining backward compatibility and existing naming conventions.

## Phase 1: Replace with Interned Structs for Common Strings

**Goal**: Replace string fields with interned versions to reduce memory usage and enable faster comparisons.

**Implementation**:
```rust
// In crates/djls-templates/src/ast.rs - REPLACE string fields with these

#[salsa::interned]
pub struct TagName<'db> {
    #[returns(ref)]
    pub text: String,
}

#[salsa::interned]
pub struct VariableName<'db> {
    #[returns(ref)]
    pub text: String,
}

#[salsa::interned]
pub struct FilterName<'db> {
    #[returns(ref)]
    pub text: String,
}
```

**TagSpecs Integration**:
- Update TagSpecs to use `TagName<'db>` for all tag name comparisons
- This makes tag validation FASTER - comparing interned IDs instead of strings
- All the tag names in TagSpecs (like "if", "for", "block") get interned once and reused

**Benefits**:
- Django templates have many repeated tag names (`if`, `for`, `block`, etc.)
- Variable names are often repeated across templates
- Filter names are from a fixed set (matches what's in TagSpecs)
- Interning these will save memory and make equality checks faster

## Phase 2: Convert AST Nodes to Tracked Structs (Using Existing Names)

**Goal**: Replace existing AST with salsa-tracked versions, keeping the same structure names.

**Implementation**:
```rust
// REPLACE existing structs with tracked versions

#[salsa::tracked]
pub struct Ast<'db> {
    #[tracked]
    #[returns(ref)]
    pub nodes: Vec<Node<'db>>,
}

#[salsa::tracked]
pub struct Span<'db> {
    #[tracked]
    pub start: usize,
    #[tracked]
    pub end: usize,
}

// Individual node types as tracked structs
#[salsa::tracked]
pub struct TagNode<'db> {  // Your existing naming pattern
    pub name: TagName<'db>,    // Using interned name from Phase 1
    #[tracked]
    #[returns(ref)]
    pub bits: Vec<String>,      // Could intern these too if beneficial
    pub span: Span<'db>,
}

#[salsa::tracked]
pub struct VariableNode<'db> {  // Your existing naming pattern
    pub var: VariableName<'db>,     // Using interned name
    #[tracked]
    #[returns(ref)]
    pub filters: Vec<FilterName<'db>>,  // Interned filter names
    pub span: Span<'db>,
}

#[salsa::tracked]
pub struct CommentNode<'db> {  // Your existing naming pattern
    #[returns(ref)]
    pub content: String,
    pub span: Span<'db>,
}

#[salsa::tracked]
pub struct TextNode<'db> {  // Your existing naming pattern
    #[returns(ref)]
    pub content: String,
    pub span: Span<'db>,
}

// Node enum uses the tracked structs
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub enum Node<'db> {
    Tag(TagNode<'db>),
    Variable(VariableNode<'db>),
    Comment(CommentNode<'db>),
    Text(TextNode<'db>),
}
```

**This is a REPLACEMENT, not addition**:
- Old non-tracked structs are completely replaced
- Same names, same structure, just salsa-aware
- All consumers updated to handle the `'db` lifetime

## Phase 3: Split `analyze_template` into Separate Tracked Functions

**Goal**: Enable independent caching and recomputation of each compilation phase.

**Implementation**:
```rust
// In crates/djls-templates/src/lib.rs

/// Lexer as a tracked struct for incremental lexing
#[salsa::tracked]
pub struct Lexer<'db> {
    pub source: SourceFile,
}

/// Parser as a tracked struct for incremental parsing
#[salsa::tracked]
pub struct Parser<'db> {
    pub source: SourceFile,
    #[tracked]
    #[returns(ref)]
    pub tokens: Arc<TokenStream>,
}

/// Phase 1: Lexical analysis
#[salsa::tracked]
pub fn lex_template(db: &dyn Db, file: SourceFile) -> Arc<TokenStream> {
    if file.kind(db) != FileKind::Template {
        return Arc::new(TokenStream::empty());
    }
    
    let text = djls_workspace::db::source_text(db, file);
    // Use existing Lexer logic - just wrap the result
    match crate::Lexer::new(&text).tokenize() {
        Ok(tokens) => Arc::new(tokens),
        Err(err) => {
            // Convert to LSP diagnostic and accumulate
            let diagnostic = /* existing error conversion logic */;
            TemplateDiagnostic(diagnostic).accumulate(db);
            Arc::new(TokenStream::empty())
        }
    }
}

/// Phase 2: Parsing
#[salsa::tracked]
pub fn parse_template(db: &dyn Db, file: SourceFile) -> Option<Ast<'_>> {
    if file.kind(db) != FileKind::Template {
        return None;
    }
    
    let tokens = lex_template(db, file);
    if tokens.is_empty() {
        return None;
    }
    
    // Create Parser tracked struct
    let parser = Parser::new(db, file, tokens);
    // Use existing parsing logic, just adapted to use db for interning
    parse_with_parser(db, parser)
}

/// Helper function that adapts existing Parser logic
#[salsa::tracked]
fn parse_with_parser(db: &dyn Db, parser: Parser<'_>) -> Option<Ast<'_>> {
    let tokens = parser.tokens(db);
    // Use existing Parser implementation, minimally modified
    // to intern strings where beneficial
    let mut parser_impl = crate::Parser::new((*tokens).clone());
    match parser_impl.parse() {
        Ok((ast, errors)) => {
            // Accumulate errors as before
            for error in errors {
                /* existing error handling */
            }
            Some(convert_to_tracked_ast(db, ast))
        }
        Err(err) => {
            /* existing error handling */
            None
        }
    }
}

/// Phase 3: Validation
#[salsa::tracked]
pub fn validate_template(db: &dyn Db, file: SourceFile) {
    let Some(ast) = parse_template(db, file) else {
        return;
    };
    
    // Use existing validation logic unchanged
    let validator = TagValidator::new(ast, db.tag_specs());
    for error in validator.validate() {
        /* existing error handling */
    }
}

/// Main entry point - orchestrates all phases
#[salsa::tracked]
pub fn analyze_template(db: &dyn Db, file: SourceFile) -> Option<Ast<'_>> {
    validate_template(db, file);
    parse_template(db, file)
}
```

**Benefits**:
- Lexer and Parser are now tracked structs for better incremental computation
- If only validation rules change, parsing isn't redone
- Lexing can be cached independently
- Each phase can be tested in isolation
- Clearer separation of concerns

## Phase 4: Integrate Salsa into Existing Parser/Lexer

**Goal**: Minimally modify existing Parser and Lexer to leverage salsa features without rewriting working logic.

**Key Changes**:
- Add database parameter to Parser/Lexer where needed for interning
- Intern strings at key points (tag names, variable names, filter names)
- Convert final AST to use tracked structs
- **Keep all existing parsing logic intact**

```rust
// Minimal changes to existing Parser
impl Parser {
    // Add new method that takes db for interning
    pub fn parse_with_db<'db>(&mut self, db: &'db dyn Db) -> Result<(Ast<'db>, Vec<ParserError>), ParserError> {
        // Existing parse logic, but when creating nodes:
        // - Intern tag names: InternedTagName::new(db, name)
        // - Intern variable names: InternedVariableName::new(db, var)
        // - Intern filter names: InternedFilterName::new(db, filter)
        // - Create tracked spans: Span::new(db, start, end)
        
        // Most of the existing logic remains unchanged
    }
}

// Helper to convert existing AST to tracked AST
fn convert_to_tracked_ast<'db>(db: &'db dyn Db, ast: Ast) -> Ast<'db> {
    // Convert existing nodes to tracked nodes
    // This is a one-time conversion at the end of parsing
}
```

## Phase 5: Update Validation to Work with New Tracked Structs

**Goal**: Minimally adapt the validator to work with the new AST structure.

**Implementation**:
- Add a simple conversion layer if needed to work with tracked AST
- Take advantage of interned strings for faster tag name comparisons (just use `.text(db)` on interned values)
- Most validation logic remains unchanged

```rust
// Minimal changes - mostly just accessing interned values
impl TagValidator {
    fn validate_with_db(&self, db: &dyn Db) -> Vec<ValidationError> {
        // When comparing tag names:
        // Before: if tag.name == "if"
        // After: if tag.name.text(db) == "if"
        
        // Rest of logic stays the same
    }
}
```

## Phase 6: Update Tests Throughout Each Phase

**Goal**: Keep tests passing as we make changes - this happens during each phase, not as a separate phase.

**Strategy**:
- **During Phase 1**: Test that interning works correctly
- **During Phase 2**: Test tracked struct creation
- **During Phase 3**: Test each phase independently
- **During Phase 4**: Ensure existing parser tests still pass
- **During Phase 5**: Ensure validation tests still pass
- Add incremental computation tests as we go

## Phase 7: Future Optimization (Optional)

**Goal**: Once everything is working, measure and optimize if needed.

**Potential future work**:
- Profile memory usage with interned strings
- Measure cache hit rates
- Fine-tune what gets tracked vs what doesn't
- Consider interning more strings (like tag arguments)

## Implementation Order and Strategy

1. **Start Small**: Begin with Phase 1 (interned structs) as it's the least disruptive
2. **Parallel Development**: Phases 2-3 can be developed in parallel with the old system still working
3. **Gradual Migration**: Use feature flags or parallel implementations during transition
4. **Backward Compatibility**: Keep the existing `analyze_template` API working throughout
5. **Testing First**: Write tests for new functionality before implementing

## Expected Benefits

1. **Memory Efficiency**: Interned strings reduce memory usage significantly
2. **Incremental Computation**: Only reparse/revalidate what changed
3. **Better Caching**: Each phase cached independently
4. **Cleaner Architecture**: Clear separation between lexing, parsing, and validation
5. **Performance**: Faster template analysis, especially for large projects
6. **Debugging**: Easier to debug issues in specific phases

## Risks and Mitigation

1. **Risk**: Increased complexity
   - **Mitigation**: Comprehensive documentation and examples

2. **Risk**: Breaking existing consumers
   - **Mitigation**: Maintain backward compatibility during transition

3. **Risk**: Performance regression in small projects
   - **Mitigation**: Benchmark before/after, optimize if needed

## Task Tracking

- [ ] Phase 1: Add interned structs for common strings (tag/variable/filter names)
- [ ] Phase 2: Convert AST nodes to tracked structs (Node -> tracked structs)
- [ ] Phase 3: Split analyze_template into separate tracked functions (lex/parse/validate)
- [ ] Phase 4: Update parser to use interned strings and tracked structs
- [ ] Phase 5: Update validation to work with new tracked structs
- [ ] Phase 6: Update tests to work with new architecture
- [ ] Phase 7: Performance testing and optimization

This plan provides a clear path to leveraging salsa's advanced features while maintaining the existing architecture and naming conventions.
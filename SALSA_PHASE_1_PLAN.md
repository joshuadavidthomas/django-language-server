# Phase 1: Interned Structs Implementation Plan

## Status: ✅ COMPLETED

## Goal
Replace string fields with salsa interned structs to reduce memory usage and enable faster comparisons throughout the Django template parsing system.

## Overview
This phase introduces interned structs for the three main types of repeated strings in Django templates:
- Tag names (e.g., "if", "for", "block", "extends")
- Variable names (e.g., "user", "request", "object")
- Filter names (e.g., "date", "truncatechars", "safe")

## File Changes Required

### 1. `crates/djls-templates/src/ast.rs`

#### Add Interned Structs
```rust
// Add at the top of the file after imports
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

#### Update Node Enum
The existing `Node` enum needs to be updated to use interned strings:

**Current:**
```rust
pub enum Node {
    Tag {
        name: String,
        bits: Vec<String>,
        span: Span,
    },
    Variable {
        var: String,
        filters: Vec<String>,
        span: Span,
    },
    // ...
}
```

**New (with 'db lifetime):**
```rust
pub enum Node<'db> {
    Tag {
        name: TagName<'db>,
        bits: Vec<String>,  // Keep as strings for now, could intern later
        span: Span,
    },
    Variable {
        var: VariableName<'db>,
        filters: Vec<FilterName<'db>>,
        span: Span,
    },
    Comment {
        content: String,  // Keep as string - not repeated
        span: Span,
    },
    Text {
        content: String,  // Keep as string - not repeated
        span: Span,
    },
}
```

#### Update Ast Struct
```rust
pub struct Ast<'db> {
    nodelist: Vec<Node<'db>>,
    line_offsets: LineOffsets,
}
```

### 2. `crates/djls-templates/src/parser.rs`

The parser needs to be updated to:
1. Accept a database reference for interning
2. Create interned strings when parsing

#### Update Parser Struct
```rust
use crate::db::Db as TemplateDb;

pub struct Parser<'db> {
    db: &'db dyn TemplateDb,  // Add database reference
    tokens: TokenStream,
    current: usize,
    errors: Vec<ParserError>,
}

impl<'db> Parser<'db> {
    pub fn new(db: &'db dyn TemplateDb, tokens: TokenStream) -> Self {
        Self {
            db,
            tokens,
            current: 0,
            errors: Vec::new(),
        }
    }
}
```

#### Update parse_django_block Method
```rust
pub fn parse_django_block(&mut self) -> Result<Node<'db>, ParserError> {
    let token = self.peek_previous()?;
    
    let args: Vec<String> = token
        .content()
        .split_whitespace()
        .map(String::from)
        .collect();
    
    let name_str = args.first().ok_or(ParserError::EmptyTag)?.clone();
    let name = TagName::new(self.db, name_str);  // Intern the tag name
    let bits = args.into_iter().skip(1).collect();
    let span = Span::from(token);
    
    Ok(Node::Tag { name, bits, span })
}
```

#### Update parse_django_variable Method
```rust
fn parse_django_variable(&mut self) -> Result<Node<'db>, ParserError> {
    let token = self.peek_previous()?;
    
    let content = token.content();
    let bits: Vec<&str> = content.split('|').collect();
    
    let var_str = bits
        .first()
        .ok_or(ParserError::EmptyTag)?
        .trim()
        .to_string();
    let var = VariableName::new(self.db, var_str);  // Intern the variable name
    
    let filters = bits
        .into_iter()
        .skip(1)
        .map(|s| FilterName::new(self.db, s.trim().to_string()))  // Intern filter names
        .collect();
    
    let span = Span::from(token);
    
    Ok(Node::Variable { var, filters, span })
}
```

### 3. `crates/djls-templates/src/validation/mod.rs`

Update the TagValidator to work with interned strings:

```rust
impl<'db> TagValidator<'db> {
    pub fn new(ast: Arc<Ast<'db>>, tag_specs: Arc<TagSpecs>) -> Self {
        // ...
    }
    
    fn validate_tag(&mut self, name: TagName<'db>, bits: &[String], span: &Span) {
        let name_str = name.text(self.db);  // Get the actual string from interned value
        
        // Rest of validation logic uses name_str
        if let Some(spec) = self.tag_specs.get(name_str) {
            // Existing validation logic
        }
    }
}
```

### 4. `crates/djls-templates/src/templatetags/specs.rs` (or tagspecs.rs)

Update TagSpecs to potentially use interned names for faster lookups:

```rust
// Option 1: Keep TagSpecs using strings but add helper for interned comparison
impl TagSpecs {
    pub fn get_by_interned<'db>(&self, name: TagName<'db>, db: &'db dyn Db) -> Option<&TagSpec> {
        self.get(name.text(db))
    }
}

// Option 2: Store interned names in TagSpecs (more complex, do later if beneficial)
```

### 5. `crates/djls-templates/src/lib.rs`

Update the main analyze_template function:

```rust
#[salsa::tracked]
pub fn analyze_template(db: &dyn Db, file: SourceFile) -> Option<Arc<Ast<'_>>> {
    if file.kind(db) != FileKind::Template {
        return None;
    }
    
    let text_arc = djls_workspace::db::source_text(db, file);
    let text = text_arc.as_ref();
    
    let tokens = match Lexer::new(text).tokenize() {
        Ok(tokens) => tokens,
        Err(err) => {
            // Error handling remains the same
            return None;
        }
    };
    
    // Pass db to parser for interning
    let (ast, parser_errors) = match Parser::new(db, tokens).parse() {
        Ok((ast, errors)) => {
            // Handle errors
            (ast, errors)
        }
        Err(err) => {
            // Error handling
            return None;
        }
    };
    
    // Validation now works with interned strings
    let validation_errors = TagValidator::new(ast.clone(), db.tag_specs()).validate();
    
    // Rest remains similar
    Some(Arc::new(ast))
}
```

### 6. `crates/djls-templates/src/db.rs`

No changes needed - the Db trait already extends WorkspaceDb which extends salsa::Database.

## Testing Strategy

### Unit Tests to Add/Update

1. **Test Interning Works**
```rust
#[test]
fn test_tag_name_interning() {
    let db = TestDatabase::default();
    let name1 = TagName::new(&db, "if".to_string());
    let name2 = TagName::new(&db, "if".to_string());
    // These should be the same interned value
    assert_eq!(name1, name2);
}
```

2. **Test Parser Creates Interned Values**
```rust
#[test]
fn test_parser_interns_tag_names() {
    let db = TestDatabase::default();
    let tokens = /* ... */;
    let parser = Parser::new(&db, tokens);
    let (ast, _) = parser.parse().unwrap();
    
    // Verify tag names are properly interned
    // Parse same template twice, verify same interned values
}
```

3. **Update Existing Tests**
- All parser tests need to pass a database
- All validation tests need to work with interned strings
- Snapshot tests may need updating if debug output changes

## Migration Steps

1. **Add interned structs to ast.rs** (compile will break)
2. **Update Node enum to use interned types** (more breakage)
3. **Update Parser to accept db and intern strings**
4. **Update validation to work with interned strings**
5. **Update analyze_template to pass db to Parser**
6. **Fix all compilation errors**
7. **Run tests and fix failures**
8. **Add new tests for interning behavior**

## Benefits After Implementation

1. **Memory Savings**: Common tag names like "if", "for", "block" stored once
2. **Faster Comparisons**: Comparing interned IDs instead of strings
3. **Cache Friendly**: Interned values work well with salsa's caching
4. **Foundation for Next Phases**: Sets up the lifetime parameter needed for tracked structs

## Implementation Notes (Actual Implementation)

### Key Discoveries and Changes

1. **Ast Became a Tracked Struct Early**
   - Originally planned for Phase 2, but we made `Ast` a tracked struct in Phase 1
   - This was necessary to resolve lifetime issues with the salsa tracked function
   - Required adding `#[salsa::tracked]` to `Ast` with `#[tracked]` and `#[returns(ref)]` on fields

2. **salsa::Update Derive Required**
   - `Node<'db>` needed `#[derive(salsa::Update)]` to work properly in tracked structs
   - This wasn't in the original plan but is essential for salsa to track changes
   - Pattern learned from salsa's calc example

3. **Interned Struct Attributes**
   - Used `#[salsa::interned]` without additional attributes (not `#[salsa::interned(debug)]`)
   - The `#[returns(ref)]` attribute wasn't needed on interned struct fields
   - Interned structs automatically implement equality and hashing

4. **Return Type Changes**
   - `analyze_template` returns `Option<Ast<'_>>` with elided lifetime, not `Option<Arc<Ast<'static>>>`
   - No `Arc` wrapper needed since tracked structs are already reference-counted by salsa
   - This matches the pattern in salsa's calc example where `parse_statements` returns `Program<'_>`

5. **Validation Borrow Checker Issues**
   - `TagValidator` couldn't hold references from `current_node()` while mutating itself
   - Solution: Changed `current_node()` to return `Option<Node<'db>>` (cloned) instead of `Option<&Node>`
   - Cloning is cheap since interned types are just IDs and Span is small

6. **Required Trait Implementations**
   - `Node<'db>`, `Span`, and `LineOffsets` needed `Hash` derive for use in tracked structs
   - All types stored in tracked structs need to implement `Hash + Eq + Debug`

7. **Database Access Pattern**
   - Tracked struct fields are accessed via methods that take `db`: `ast.nodelist(db)`
   - This is different from regular struct field access
   - Validator needed to pass `self.db` when accessing AST fields

### Lessons Learned

1. **Follow the Calc Example**: The salsa calc example provides the canonical patterns for:
   - Tracked vs non-tracked structs
   - How to handle lifetimes
   - When to use `#[tracked]` and `#[returns(ref)]` attributes

2. **Lifetime Management**: 
   - Tracked structs with lifetime parameters work but have constraints
   - The elided lifetime `'_` in return types is the correct pattern for tracked functions
   - Don't try to use `'static` - it creates impossible lifetime constraints

3. **Incremental Migration**:
   - It's okay to do Phase 1 and parts of Phase 2 together when it makes sense
   - The tracked struct conversion helped resolve lifetime issues early

4. **Test Updates Required**:
   - Parser tests need to create a database and pass it to `Parser::new()`
   - Validation tests need similar updates
   - Line offset access in tests needs to use `ast.line_offsets(db)` not `ast.line_offsets()`

## Potential Issues and Solutions

### Issue 1: Lifetime Complexity
- **Problem**: Adding `'db` lifetime to many structs
- **Solution**: This is necessary for salsa integration, helps with incremental computation

### Issue 2: Test Updates
- **Problem**: Many tests will need updating to provide database
- **Solution**: Create a test helper that sets up a database with common configuration

### Issue 3: Debug Output Changes
- **Problem**: Interned structs may have different Debug output
- **Solution**: Implement custom Debug that shows the text value for clarity

## Success Criteria

- [ ] All tag names are interned
- [ ] All variable names are interned  
- [ ] All filter names are interned
- [ ] Parser successfully creates interned values
- [ ] Validation works with interned values
- [ ] All existing tests pass
- [ ] Memory usage reduced for templates with repeated names
- [ ] No performance regression

## Next Steps After Phase 1

Once interning is working:
- Phase 2: Convert AST nodes to tracked structs
- Phase 3: Split analyze_template into separate phases
- Phase 4: Make Parser and Lexer tracked structs

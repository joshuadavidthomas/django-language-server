# Salsa Optimization Plan for djls-semantic

## Overview

Transform djls-semantic to leverage Salsa's advanced patterns learned from Ruff's architecture, focusing on interning, cycle recovery, and optimal tracking granularity. This plan outlines a comprehensive refactoring to achieve performance parity with state-of-the-art language servers.

## Key Lessons from Ruff's Architecture

1. **Enum with Tracked Impl Pattern**: Separate data representation from computation
2. **Interned Types for Deduplication**: Reduce memory usage and enable fast equality checks
3. **The #[no_eq] Pattern**: Exclude spans from equality to avoid reformatting invalidation
4. **Cycle Recovery**: Handle recursive definitions gracefully with fixed-point iteration
5. **Return Optimizations**: Avoid expensive clones with strategic return modifiers
6. **Shallow Salsa Integration**: Thin tracked wrappers delegating to pure logic
7. **Strategic Tracking**: Only track expensive computations, not trivial accessors

## Architecture: The Three-Layer Pattern

Based on Ruff's approach, we'll use a shallow Salsa integration with three distinct layers:

```
┌─────────────────────────────────────────┐
│  Salsa Layer (Thin)                     │  <- Tracked functions & methods (10-20 lines)
├─────────────────────────────────────────┤
│  Builder/Orchestration Layer            │  <- AnalysisBuilder, InheritanceResolver (1000+ lines)
├─────────────────────────────────────────┤
│  Core Computation Layer                 │  <- Core algorithms (testable/benchmarkable)
└─────────────────────────────────────────┘
```

### Layer 1: Salsa Tracked Functions (Thin Wrappers)

```rust
// Thin wrapper - just orchestration
#[salsa::tracked]
pub fn analyze_template<'db>(
    db: &'db dyn Db,
    nodelist: NodeList<'db>,
) -> AnalysisBundle<'db> {
    let specs = db.tag_specs();
    let nodes = nodelist.nodelist(db);
    
    // Delegate to builder - no actual logic here!
    AnalysisBuilder::analyze(nodes, &specs)
        .with_interning(db)
        .finish()
}
```

### Layer 2: Builder/Orchestration (Where Logic Lives)

```rust
pub struct AnalysisBuilder {
    // Mutable state during construction
    semantic_nodes: Vec<SemanticElement>,
    errors: Vec<ValidationError>,
    // ... more state
}

impl AnalysisBuilder {
    /// Core analysis logic - can be tested without Salsa
    pub fn analyze(nodes: &[Node], specs: &TagSpecs) -> Self {
        // 1000+ lines of actual analysis logic
        // No database access, just computation
    }
}
```

### Layer 3: Core Computation Functions

```rust
// Core validation logic - no DB needed
pub fn validate_tag_arguments(
    tag_name: &str,
    arguments: &[String],
    spec: &TagSpec,
) -> Vec<ValidationError> {
    // Core algorithm, fully testable
}

// Core type inference
pub fn infer_variable_type_impl(
    context: &TemplateContext,
    variable_path: &[String],
) -> Type {
    // Complex logic but no DB access
}
```

## When to Track: Decision Guide

```
Should this be a tracked function/method?
├─ Is it expensive (>1ms)?
│  ├─ YES → Does it have deterministic output?
│  │  ├─ YES → Is it called frequently?
│  │  │  ├─ YES → TRACK IT (with delegation to pure logic)
│  │  │  └─ NO → Maybe track (measure first)
│  │  └─ NO → DON'T TRACK (non-deterministic)
│  └─ NO → DON'T TRACK (too cheap to cache)
```

### Examples of What to Track vs Not Track

```rust
#[salsa::tracked]
impl<'db> SemanticElement<'db> {
    // TRACK: Expensive validation
    pub fn validate(self, db: &'db dyn Db) -> Vec<ValidationError> {
        // Delegate to implementation
        validation::validate_element_impl(db.tag_specs(), self)
    }
    
    // TRACK: Complex type inference
    pub fn inferred_type(self, db: &'db dyn Db) -> Option<Type<'db>> {
        type_inference::infer_element_type(self, db.context())
    }
    
    // DON'T TRACK: Simple field access (make this a regular method)
    pub fn span(&self) -> Span {
        match self {
            Self::Tag(t) => t.span,
            Self::Variable(v) => v.span,
        }
    }
    
    // DON'T TRACK: Trivial check (regular method)
    pub fn is_tag(&self) -> bool {
        matches!(self, Self::Tag(_))
    }
}
```

## Testing & Benchmarking Strategy

### Three Levels of Testing

```rust
// Level 1: Core functions (no Salsa, no DB)
#[test]
fn test_validation_logic() {
    let spec = create_test_spec();
    let errors = validate_tag_arguments("for", &["item", "in"], &spec);
    assert!(errors.is_empty());
}

// Level 2: Builders (orchestration logic)
#[test]
fn test_analysis_builder() {
    let builder = AnalysisBuilder::new_for_testing();
    let result = builder.analyze(&nodes, &specs);
    assert_eq!(result.semantic_elements.len(), 5);
}

// Level 3: Full integration (with Salsa caching)
#[test]
fn test_tracked_analysis() {
    let db = TestDatabase::new();
    let bundle = analyze_template(&db, nodelist);
    
    // Test caching behavior
    let bundle2 = analyze_template(&db, nodelist);
    assert!(std::ptr::eq(&bundle, &bundle2)); // Same cached result
}
```

### Benchmarking Suite

```rust
#[bench]
fn bench_analysis_impl(b: &mut Bencher) {
    // Benchmark JUST the algorithm, no Salsa overhead
    let nodes = create_test_nodes();
    let specs = create_test_specs();
    
    b.iter(|| {
        AnalysisBuilder::analyze(&nodes, &specs)
    });
}

#[bench]
fn bench_with_salsa_cold(b: &mut Bencher) {
    // Include Salsa overhead, no cache hits
    b.iter_batched(
        || TestDatabase::new_with_template(template),
        |db| analyze_template(&db, nodelist),
        BatchSize::SmallInput,
    );
}

#[bench]
fn bench_with_salsa_warm(b: &mut Bencher) {
    // Measure cache hit performance
    let db = TestDatabase::new();
    analyze_template(&db, nodelist); // Warm cache
    
    b.iter(|| analyze_template(&db, nodelist));
}
```

## Common Patterns

### Pattern 1: Thin Tracked Wrapper + Core Implementation

```rust
#[salsa::tracked]
pub fn validate_template<'db>(db: &'db dyn Db, nodelist: NodeList<'db>) -> Vec<ValidationError> {
    // Just orchestration
    let specs = db.tag_specs();
    let nodes = nodelist.nodelist(db);
    
    // Delegate to core function
    validation::validate_nodes(&nodes, &specs)
}
```

### Pattern 2: Tracked Method with Delegation

```rust
#[salsa::tracked]
impl<'db> TagName<'db> {
    // Track expensive spec resolution
    pub fn resolve_spec(self, db: &'db dyn Db) -> Option<TagSpec> {
        spec_resolution::resolve(db.tag_specs(), self.text(db))
    }
    
    // Don't track simple getter - regular method
    pub fn as_str(self, db: &'db dyn Db) -> &str {
        self.text(db)
    }
}
```

### Pattern 3: Builder Pattern with Core Logic

```rust
pub struct InheritanceResolver<'db> {
    db: &'db dyn Db,
    visited: FxHashSet<TemplatePath<'db>>,
}

impl<'db> InheritanceResolver<'db> {
    pub fn resolve(&mut self, template: TemplatePath<'db>) -> Resolution {
        // Complex logic, but no DB calls in algorithm
    }
}

// Tracked entry point
#[salsa::tracked]
pub fn resolve_inheritance<'db>(
    db: &'db dyn Db,
    template: TemplatePath<'db>,
) -> ResolvedTemplate<'db> {
    InheritanceResolver::new(db)
        .resolve(template)
        .build()
}
```

## Existing Foundation: djls-source Types

The djls-source crate already provides a robust foundation for position tracking and file management that we'll build upon:

### Core Types from djls-source

```rust
// Already available in djls-source:
pub struct Span {
    start: u32,
    length: u32,
}

pub struct Offset(u32);

pub struct LineCol {
    line: u32,
    column: u32,
}

#[salsa::input]
pub struct File {
    pub path: Utf8PathBuf,
    pub revision: u64,
}

pub struct SourceText(Arc<SourceTextInner>);

pub struct LineIndex(Vec<u32>);
```

### Integration Strategy with Salsa Patterns

#### 1. Using Span with #[no_eq] Pattern

```rust
// crates/djls-semantic/src/semantic_types.rs
use djls_source::{Span, Offset, File};

#[salsa::tracked]
pub struct SemanticTag<'db> {
    pub id: SemanticId,
    pub name: TagName<'db>,
    pub arguments: ArgumentList<'db>,
    
    // Use existing Span type but exclude from equality
    #[no_eq]
    #[tracked]
    pub span: Span,  // From djls_source
    
    #[no_eq]
    #[tracked]
    pub closing_span: Option<Span>,
}
```

#### 2. File-Based Queries

```rust
// Leverage existing File input type
#[salsa::tracked]
pub fn analyze_template_file<'db>(
    db: &'db dyn Db,
    file: File,  // From djls_source
) -> TemplateAnalysis<'db> {
    let source = file.source(db);
    let line_index = file.line_index(db);
    // ... analysis using existing infrastructure
}
```

#### 3. Position Conversion Utilities

```rust
// crates/djls-semantic/src/position_utils.rs

/// Convert offset to semantic element using existing types
pub fn element_at_offset<'db>(
    db: &'db dyn Db,
    file: File,
    offset: Offset,  // From djls_source
) -> Option<SemanticElement<'db>> {
    let line_index = file.line_index(db);
    let line_col = line_index.to_line_col(offset);
    
    // Use existing position infrastructure
    let analysis = analyze_template_file(db, file);
    analysis.find_at_offset(offset.get())
}

/// Get span with expanded delimiters
pub fn expand_tag_span(span: Span, delimiter_length: u32) -> Span {
    // Use existing Span::expand method
    span.expand(delimiter_length, delimiter_length)
}
```

#### 4. Interned Types That Reference Files

```rust
#[salsa::interned]
pub struct TemplateReference<'db> {
    pub file: File,  // Reference to djls_source::File
    pub path: TemplatePath<'db>,
}

#[salsa::tracked]
pub struct ResolvedTemplate<'db> {
    pub file: File,  // From djls_source
    pub parent: Option<ResolvedTemplate<'db>>,
    
    #[no_eq]
    #[tracked]
    pub source_span: Span,  // Position in source
}
```

### Key Integration Points

1. **Span Handling**: Use `djls_source::Span` everywhere, apply `#[no_eq]` pattern to avoid reformatting invalidation
2. **File Management**: Leverage `File` as input with revision tracking for incremental updates
3. **Line Indexing**: Use existing `LineIndex` for offset↔line/col conversions
4. **Source Text**: Use `SourceText` for efficient text storage with encoding detection

### Migration Notes

- **DO NOT** duplicate position types - use `djls_source::{Span, Offset, LineCol}`
- **DO** add `#[no_eq]` to spans in tracked structs to prevent position-based invalidation
- **DO** use `File` as the primary input for template analysis
- **DO** leverage `LineIndex` for all position conversions

This ensures we build on the solid foundation already in place rather than reinventing position tracking.

## Core Function Extraction Pattern

A key principle: extract complex logic into standalone functions that can be tested and benchmarked without Salsa.

### Example: Validation

```rust
// crates/djls-semantic/src/validation/impl.rs

/// Core validation logic - no DB, fully testable
pub fn validate_tag_arguments(
    tag_name: &str,
    arguments: &[String],
    spec: &TagSpec,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    
    // Complex validation logic here
    if arguments.len() < spec.min_args {
        errors.push(ValidationError::TooFewArguments { 
            tag: tag_name.to_string(),
            min: spec.min_args,
        });
    }
    
    // More validation...
    errors
}

// crates/djls-semantic/src/validation/mod.rs

/// Thin tracked wrapper for caching
#[salsa::tracked]
pub fn validate_tag<'db>(
    db: &'db dyn Db,
    tag: SemanticTag<'db>,
) -> Vec<ValidationError> {
    let spec = db.tag_specs().get(tag.name(db).text(db));
    
    // Just delegate to core function
    validate_tag_arguments(
        tag.name(db).text(db),
        tag.arguments(db).args(db),
        spec.unwrap_or(&DEFAULT_SPEC),
    )
}
```

### Example: Type Inference

```rust
// Core type inference implementation
pub fn infer_variable_type_impl(
    variable_path: &[String],
    context: &TemplateContext,
    loop_scopes: &[LoopScope],
) -> Type {
    // Complex type inference logic
    // No DB access, just computation
    
    // Check loop variables
    for scope in loop_scopes {
        if scope.iterator_name == variable_path[0] {
            return scope.item_type.clone();
        }
    }
    
    // Check context
    context.get_type(variable_path).unwrap_or(Type::Any)
}

// Tracked wrapper
#[salsa::tracked]
pub fn infer_variable<'db>(
    db: &'db dyn Db,
    var: SemanticVariable<'db>,
    template: ResolvedTemplate<'db>,
) -> Type<'db> {
    let context = template.context(db);
    let loops = template.loop_scopes(db);
    
    // Delegate to core implementation
    infer_variable_type_impl(
        var.path(db).segments(db),
        context,
        loops,
    )
}
```

## Implementation Phases

### Phase 1: Foundation - Interning Infrastructure (Week 1)

#### 1.1 Create Interned Types Module (`crates/djls-semantic/src/interned.rs`)

```rust
// Note: We use the existing Span type from djls_source throughout
use djls_source::{Span, Offset, File};

/// Interned tag name for deduplication
#[salsa::interned]
pub struct TagName<'db> {
    #[return_ref]
    pub text: String,
}

/// Interned variable path (e.g., user.profile.name)
#[salsa::interned]
pub struct VariablePath<'db> {
    #[return_ref]
    pub segments: Vec<String>,
}

/// Interned template path for extends/includes
#[salsa::interned]
pub struct TemplatePath<'db> {
    #[return_ref]
    pub path: String,
}

/// Interned argument list for tag arguments
#[salsa::interned]
pub struct ArgumentList<'db> {
    #[return_ref]
    pub args: Vec<String>,
}

/// Interned filter chain for variables
#[salsa::interned]
pub struct FilterChain<'db> {
    #[return_ref]
    pub filters: Vec<FilterCall>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FilterCall {
    pub name: String,
    pub args: Vec<String>,
}
```

#### 1.2 Update Database Trait

```rust
// crates/djls-semantic/src/db.rs
#[salsa::db]
pub trait Db: djls_templates::Db {
    fn tag_specs(&self) -> TagSpecs;
    fn tag_index(&self) -> TagIndex;
    
    // New: Interning support
    #[salsa::interned]
    fn intern_tag_name(&self, name: TagName<'_>) -> TagName<'_>;
    
    #[salsa::interned]
    fn intern_variable_path(&self, path: VariablePath<'_>) -> VariablePath<'_>;
    
    #[salsa::interned]
    fn intern_template_path(&self, path: TemplatePath<'_>) -> TemplatePath<'_>;
    
    #[salsa::interned]
    fn intern_argument_list(&self, args: ArgumentList<'_>) -> ArgumentList<'_>;
    
    #[salsa::interned]
    fn intern_filter_chain(&self, chain: FilterChain<'_>) -> FilterChain<'_>;
}
```

### Phase 2: Span-Aware Semantic Model (Week 1-2)

#### 2.1 Refactor Core Types with #[no_eq] Pattern

```rust
// crates/djls-semantic/src/semantic_types.rs
use djls_source::{Span, Offset, File};  // Use existing position types

/// Semantic representation of a tag
#[salsa::tracked]
pub struct SemanticTag<'db> {
    pub id: SemanticId,
    pub name: TagName<'db>,        // Interned
    pub arguments: ArgumentList<'db>, // Interned
    
    #[no_eq]  // Exclude from equality - position doesn't affect semantics
    #[tracked]
    #[return_ref]
    pub span: Span,  // From djls_source
    
    #[no_eq]
    #[tracked]
    #[return_ref]
    pub closing_span: Option<Span>,
}

/// Semantic representation of a variable
#[salsa::tracked]
pub struct SemanticVariable<'db> {
    pub path: VariablePath<'db>,    // Interned
    pub filters: FilterChain<'db>,  // Interned
    
    #[no_eq]
    #[tracked]
    #[return_ref]
    pub span: Span,
}

/// Main semantic element enum
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum SemanticElement<'db> {
    Tag(SemanticTag<'db>),
    Variable(SemanticVariable<'db>),
    Text(TextNode<'db>),
    Block(BlockNode<'db>),
}

// Tracked methods ONLY for expensive computations
#[salsa::tracked]
impl<'db> SemanticElement<'db> {
    /// Get documentation for hover (expensive - may load external files)
    pub fn documentation(self, db: &'db dyn Db) -> Option<String> {
        // Delegate to implementation
        documentation::resolve_element_docs(self, db.tag_specs())
    }
    
    /// Validate element (expensive - complex rules)
    pub fn validate(self, db: &'db dyn Db) -> Vec<ValidationError> {
        // Delegate to validation logic
        validation::validate_element(self, db.tag_specs())
    }
    
    /// Type inference for variables (expensive)
    pub fn inferred_type(self, db: &'db dyn Db) -> Option<Type<'db>> {
        match self {
            Self::Variable(var) => {
                Some(type_inference::infer_variable_type(var, db.context()))
            }
            _ => None
        }
    }
}

// Regular methods for simple operations (NOT tracked)
impl<'db> SemanticElement<'db> {
    // Simple accessor - no tracking needed
    pub fn span(&self) -> Span {
        match self {
            Self::Tag(t) => t.span,
            Self::Variable(v) => v.span,
            _ => Span::default(),
        }
    }
    
    // Trivial check - no tracking needed
    pub fn is_tag(&self) -> bool {
        matches!(self, Self::Tag(_))
    }
}
```

### Phase 3: Cycle-Aware Template Resolution (Week 2)

#### 3.1 Template Inheritance System

```rust
// crates/djls-semantic/src/inheritance.rs

/// Resolved template with inheritance chain
#[salsa::tracked]
pub struct ResolvedTemplate<'db> {
    pub path: TemplatePath<'db>,
    pub parent: Option<ResolvedTemplate<'db>>,
    pub blocks: BlockMap<'db>,
}

/// Block inheritance resolution
#[salsa::tracked(
    cycle_fn=block_cycle_recover,
    cycle_initial=block_cycle_initial
)]
pub fn resolve_block<'db>(
    db: &'db dyn Db,
    template: ResolvedTemplate<'db>,
    block_name: TagName<'db>,
) -> ResolvedBlock<'db> {
    // First check local blocks
    if let Some(local_block) = template.blocks(db).get(&block_name) {
        if !local_block.is_super() {
            return ResolvedBlock::new(db, local_block, None);
        }
    }
    
    // Then check parent
    if let Some(parent) = template.parent(db) {
        let parent_block = resolve_block(db, parent, block_name);
        ResolvedBlock::new(db, local_block, Some(parent_block))
    } else {
        ResolvedBlock::NotFound
    }
}

fn block_cycle_recover<'db>(
    _db: &'db dyn Db,
    _template: ResolvedTemplate<'db>,
    _block_name: TagName<'db>,
) -> salsa::CycleRecoveryAction<ResolvedBlock<'db>> {
    // Cycle in inheritance - return empty block
    salsa::CycleRecoveryAction::Fallback(ResolvedBlock::Empty)
}

fn block_cycle_initial<'db>(
    _db: &'db dyn Db,
    _template: ResolvedTemplate<'db>,
    _block_name: TagName<'db>,
) -> ResolvedBlock<'db> {
    ResolvedBlock::Empty
}

/// Template include resolution
#[salsa::tracked(
    cycle_fn=include_cycle_recover,
    cycle_initial=include_cycle_initial
)]
pub fn resolve_include<'db>(
    db: &'db dyn Db,
    template: ResolvedTemplate<'db>,
    include_path: TemplatePath<'db>,
    context: IncludeContext<'db>,
) -> ResolvedInclude<'db> {
    // Check for circular includes
    if context.chain(db).contains(&include_path) {
        return ResolvedInclude::Circular;
    }
    
    // Load and resolve included template
    let included = load_template(db, include_path);
    let new_context = context.with_parent(db, template);
    
    ResolvedInclude::new(db, included, new_context)
}
```

### Phase 4: Advanced Analysis Bundle (Week 2-3)

#### 4.1 Enhanced AnalysisBundle with Pure Core and Interning

```rust
// crates/djls-semantic/src/analysis.rs

/// Analysis builder with pure core logic
pub struct AnalysisBuilder {
    // Mutable state during construction (no DB references)
    block_tree: BlockTreeBuilder,
    offset_index: OffsetIndexBuilder,
    semantic_nodes: Vec<SemanticNodeData>,
    errors: Vec<ValidationError>,
    
    // Temporary caches for interning
    tag_names: FxHashSet<String>,
    var_paths: FxHashSet<Vec<String>>,
}

impl AnalysisBuilder {
    /// Core analysis - can be tested without Salsa or database
    pub fn analyze(nodes: &[Node], specs: &TagSpecs) -> Self {
        let mut builder = Self::new(specs);
        
        for node in nodes {
            builder.visit(node);
        }
        
        builder
    }
    
    /// Add interning - requires DB but minimal logic
    pub fn with_interning<'db>(self, db: &'db dyn Db) -> AnalysisBundle<'db> {
        // Intern all collected strings
        let interned_names = self.tag_names.into_iter()
            .map(|name| (name.clone(), TagName::new(db, name)))
            .collect();
            
        // Convert to final format with interned values
        AnalysisBundle::from_inner(self, interned_names)
    }
}
    
    fn visit(&mut self, node: &Node) {
        match node {
            Node::Tag { name, bits, span } => {
                // Collect for later interning
                self.tag_names.insert(name.clone());
                
                // Create semantic data
                let node_data = SemanticNodeData::Tag {
                    name: name.clone(),
                    arguments: bits.clone(),
                    span: *span,
                };
                
                self.semantic_nodes.push(node_data);
            }
            Node::Variable { var, filters, span } => {
                // Parse and collect for interning
                let segments: Vec<String> = var.split('.').map(String::from).collect();
                self.var_paths.insert(segments.clone());
                
                let node_data = SemanticNodeData::Variable {
                    path: segments,
                    filters: filters.clone(),
                    span: *span,
                };
                
                self.semantic_nodes.push(node_data);
            }
            _ => {}
        }
    }
}

// Inner data structure for testing
#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisResultInner {
    pub block_tree: BlockTreeInner,
    pub semantic_nodes: Vec<SemanticNodeData>,
    pub offset_index: OffsetIndex,
    pub errors: Vec<ValidationError>,
}

// Salsa-aware bundle with interned values
#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisBundle<'db> {
    pub block_tree: BlockTreeInner,
    pub semantic_elements: Vec<SemanticElement<'db>>,
    pub offset_index: OffsetIndex,
    pub template_deps: Vec<TemplateDependency<'db>>,
    pub construction_errors: Vec<ValidationError>,
}
```

### Phase 5: Type System for Variables (Week 3-4)

#### 5.1 Variable Type Inference

```rust
// crates/djls-semantic/src/types.rs

/// Python-like type system for template variables
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum Type<'db> {
    Any,
    None,
    String,
    Int,
    Float,
    Bool,
    List(Box<Type<'db>>),
    Dict(DictType<'db>),
    Object(ObjectType<'db>),
    Union(UnionType<'db>),
}

/// Object type with known attributes
#[salsa::interned]
pub struct ObjectType<'db> {
    pub name: String,
    #[return_ref]
    pub attributes: FxHashMap<String, Type<'db>>,
}

/// Union of multiple types
#[salsa::interned]
pub struct UnionType<'db> {
    #[return_ref]
    pub types: Vec<Type<'db>>,
}

/// Type inference for variables
#[salsa::tracked(
    return_ref,
    cycle_fn=type_inference_cycle_recover,
    cycle_initial=type_inference_cycle_initial
)]
pub fn infer_variable_type<'db>(
    db: &'db dyn Db,
    template: ResolvedTemplate<'db>,
    var_path: VariablePath<'db>,
) -> Type<'db> {
    // Check context variables
    if let Some(context_type) = template.context(db).get(&var_path) {
        return context_type;
    }
    
    // Check for loop variables
    if let Some(loop_type) = check_loop_variable(db, template, var_path) {
        return loop_type;
    }
    
    // Check parent template context
    if let Some(parent) = template.parent(db) {
        return infer_variable_type(db, parent, var_path);
    }
    
    Type::Any
}

fn type_inference_cycle_recover<'db>(
    _db: &'db dyn Db,
    _template: ResolvedTemplate<'db>,
    _var_path: VariablePath<'db>,
) -> salsa::CycleRecoveryAction<Type<'db>> {
    salsa::CycleRecoveryAction::Fallback(Type::Any)
}
```

### Phase 6: Optimized Queries (Week 4)

#### 6.1 Query Organization

```rust
// crates/djls-semantic/src/queries.rs

/// Primary analysis query with interning
#[salsa::tracked(heap_size)]
pub fn analyze_template<'db>(
    db: &'db dyn Db,
    nodelist: djls_templates::NodeList<'db>,
) -> AnalysisBundle<'db> {
    let nodes = nodelist.nodelist(db);
    let specs = db.tag_specs();
    AnalysisBuilder::analyze(db, nodes, &specs)
}

/// Find element at offset - with type inference
#[salsa::tracked]
pub fn find_element_at_offset<'db>(
    db: &'db dyn Db,
    template: ResolvedTemplate<'db>,
    offset: u32,
) -> Option<TypedElement<'db>> {
    let bundle = analyze_template(db, template.nodelist(db));
    
    if let Some(element) = bundle.offset_index.find_at(offset) {
        match element {
            SemanticElement::Variable(var) => {
                let var_type = infer_variable_type(db, template, var.path(db));
                Some(TypedElement::Variable {
                    var,
                    inferred_type: var_type,
                })
            }
            _ => Some(TypedElement::from(element)),
        }
    } else {
        None
    }
}

/// Get all variables with types in scope
#[salsa::tracked(return_ref)]
pub fn variables_in_scope<'db>(
    db: &'db dyn Db,
    template: ResolvedTemplate<'db>,
    offset: u32,
) -> FxHashMap<VariablePath<'db>, Type<'db>> {
    let mut vars = FxHashMap::default();
    
    // Collect from current template
    let local_vars = collect_local_variables(db, template, offset);
    vars.extend(local_vars);
    
    // Collect from parent templates
    if let Some(parent) = template.parent(db) {
        let parent_vars = variables_in_scope(db, parent, u32::MAX);
        vars.extend(parent_vars);
    }
    
    vars
}
```

### Phase 7: Memory and Performance Optimization (Week 5)

#### 7.1 Memory Tracking

```rust
// Add to all tracked/interned types
impl GetSize for SemanticTag<'_> {
    fn get_heap_size(&self) -> usize {
        // Custom implementation
    }
}

// Database configuration
#[salsa::db]
pub struct TemplateDatabase {
    // ... other fields ...
    
    // MUST be last for proper drop order
    storage: salsa::Storage<TemplateDatabase>,
}
```

#### 7.2 Return Optimizations

```rust
pub struct LargeAnalysisResult<'db> {
    #[return_ref]
    pub elements: Vec<SemanticElement<'db>>,
    
    #[return_deref]
    pub tree: Box<BlockTreeInner>,
    
    #[return_as_ref]
    pub optional_data: Option<ExpensiveComputation>,
}
```

### Phase 8: Testing Strategy (Week 5-6)

#### 8.1 Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_interning_deduplication() {
        let db = TestDatabase::new();
        
        let name1 = TagName::new(&db, "block".to_string());
        let name2 = TagName::new(&db, "block".to_string());
        
        // Should be the same interned value
        assert!(name1.as_id() == name2.as_id());
    }
    
    #[test]
    fn test_cycle_recovery() {
        let db = TestDatabase::new();
        
        // Create circular template inheritance
        let template_a = create_template(&db, "a.html", "{% extends 'b.html' %}");
        let template_b = create_template(&db, "b.html", "{% extends 'a.html' %}");
        
        // Should not panic, should recover gracefully
        let resolved = resolve_template(&db, template_a);
        assert_eq!(resolved.parent(&db), None);
    }
    
    #[test]
    fn test_span_exclusion() {
        let db = TestDatabase::new();
        
        // Create same tag at different positions
        let tag1 = SemanticTag::new(&db, id, name, args, Span::new(0, 10));
        let tag2 = SemanticTag::new(&db, id, name, args, Span::new(20, 30));
        
        // Should be equal despite different spans
        assert_eq!(tag1.semantic_eq(&db), tag2.semantic_eq(&db));
    }
}
```

#### 8.2 Integration Tests

```rust
#[test]
fn test_full_template_analysis() {
    let db = TestDatabase::new();
    let template = r#"
        {% extends "base.html" %}
        {% block content %}
            {% for user in users %}
                {{ user.name|title }}
            {% endfor %}
        {% endblock %}
    "#;
    
    let file = create_file(&db, "test.html", template);
    let nodelist = parse_template(&db, file).unwrap();
    let index = SemanticIndex::new(&db, nodelist);
    
    // Test variable type inference
    let var_type = index.variable_type_at_offset(100);
    assert_eq!(var_type, Some(Type::Object(user_type)));
    
    // Test inheritance resolution
    let blocks = index.resolved_blocks();
    assert!(blocks.contains_key(&TagName::new(&db, "content")));
}
```

## Implementation Schedule

| Week | Phase | Deliverables |
|------|-------|-------------|
| 1 | Foundation | Interned types, updated DB trait |
| 1-2 | Span-Aware Model | Refactored semantic types with #[no_eq] |
| 2 | Cycle Resolution | Template inheritance with cycle recovery |
| 2-3 | Analysis Bundle | Enhanced single-pass analysis with interning |
| 3-4 | Type System | Variable type inference |
| 4 | Optimized Queries | Refactored query layer |
| 5 | Performance | Memory tracking, return optimizations |
| 5-6 | Testing | Comprehensive test suite |

## Migration Strategy

1. **Keep existing API stable**: Mark old functions as deprecated
2. **Parallel implementation**: Build new system alongside old
3. **Gradual cutover**: Switch one feature at a time
4. **Performance validation**: Benchmark before/after each phase

## Success Metrics

- **Performance**: 50% reduction in recomputation on reformatting
- **Memory**: 30% reduction through interning
- **Correctness**: Handle all template inheritance cycles
- **Type Coverage**: Infer types for 80% of common variables
- **API Stability**: Zero breaking changes for consumers
- **Testability**: 80% of logic in core functions (testable without Salsa)
- **Benchmarkability**: Can benchmark core algorithms without caching overhead
- **Cache Granularity**: Track at the right level (expensive operations only)

## Risk Mitigation

1. **Complexity**: Start with simple interning, add features gradually
2. **Performance regression**: Benchmark each phase
3. **Cycle handling**: Extensive testing of edge cases
4. **Memory leaks**: Use Salsa's garbage collection properly

## Decision Guide: When to Use Each Pattern

### Decision Tree

```
Is it external data (files, config)?
├─ YES → Use #[salsa::input]
└─ NO → Is it a computation result?
    ├─ YES → Is it a discriminated union/enum?
    │   ├─ YES → Use enum with #[salsa::tracked] impl
    │   └─ NO → Use #[salsa::tracked] function/struct
    └─ NO → Is it immutable data that appears frequently?
        ├─ YES → Need expensive computations on it?
        │   ├─ YES → Use #[salsa::interned] + tracked methods
        │   └─ NO → Use #[salsa::interned] alone
        └─ NO → Use regular Rust struct/enum

Does the computation have cycles?
├─ YES → Use cycle_fn + cycle_initial
└─ NO → Use simple #[salsa::tracked]

Does it contain AST nodes?
├─ YES → Use #[no_eq] + #[tracked] + #[returns(ref)]
└─ NO → Standard equality is fine

Is the data large?
├─ YES → Use appropriate #[returns(...)] modifier
└─ NO → Default return is fine
```

### Key Principles

1. **Separation of Concerns**: Data representation vs computation, identity vs behavior
2. **Performance First**: Track memory, avoid clones, document critical paths
3. **Cycle Awareness**: Python's dynamic nature creates cycles - handle them
4. **Incremental Design**: Break computations into logical units
5. **AST Handling**: Position changes shouldn't trigger recomputation

### Anti-Patterns to Avoid

1. **Don't track everything**: Only track expensive computations
2. **Don't ignore cycles**: Template systems have cycles, handle them
3. **Don't clone large data**: Use return modifiers
4. **Don't mix concerns**: Separate data from computation
5. **Don't forget memory tracking**: All Salsa types should track heap size

## Conclusion

This plan provides a complete roadmap to transform djls-semantic into a Salsa-optimized system that matches Ruff's sophistication while being tailored to Django template needs. The architecture:

1. **Builds on the existing djls-source foundation** for position tracking and file management
2. **Applies advanced Salsa patterns** (interning, #[no_eq], cycle recovery) to the existing types
3. **Leverages Salsa's power** not by using it everywhere, but by using the right pattern in the right place

The key is that we don't reinvent what's already working well (Span, File, LineIndex) but enhance it with Ruff's proven patterns for incremental computation and memory efficiency. Core logic remains testable and benchmarkable by extracting it into standalone functions that delegate from thin Salsa wrappers.
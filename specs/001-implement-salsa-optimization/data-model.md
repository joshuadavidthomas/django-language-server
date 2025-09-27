# Data Model: Salsa Optimization for djls-semantic

## Core Interned Types

### TagName
**Purpose**: Deduplicated storage of template tag names
**Fields**:
- `text: String` - The actual tag name (e.g., "block", "for", "extends")
**Relationships**: Referenced by SemanticTag, BlockNode
**Validation**: Must be valid Django tag name
**Salsa**: `#[salsa::interned]`

### VariablePath  
**Purpose**: Deduplicated storage of variable access paths
**Fields**:
- `segments: Vec<String>` - Path segments (e.g., ["user", "profile", "name"])
**Relationships**: Referenced by SemanticVariable
**Validation**: Each segment must be valid Python identifier
**Salsa**: `#[salsa::interned]`

### TemplatePath
**Purpose**: Deduplicated storage of template file paths
**Fields**:
- `path: String` - Relative template path (e.g., "base.html", "includes/header.html")
**Relationships**: Referenced by ResolvedTemplate, IncludeNode, ExtendsNode
**Validation**: Must be valid file path
**Salsa**: `#[salsa::interned]`

### ArgumentList
**Purpose**: Deduplicated storage of tag arguments
**Fields**:
- `args: Vec<String>` - Ordered argument list
**Relationships**: Referenced by SemanticTag
**Validation**: Arguments must be valid for the associated tag
**Salsa**: `#[salsa::interned]`

### FilterChain
**Purpose**: Deduplicated storage of filter pipelines
**Fields**:
- `filters: Vec<FilterCall>` - Ordered filter applications
**Relationships**: Referenced by SemanticVariable
**Validation**: Each filter must be known/valid
**Salsa**: `#[salsa::interned]`

## Tracked Semantic Types

### SemanticTag
**Purpose**: Represents a template tag with position-independent semantics
**Fields**:
- `id: SemanticId` - Unique identifier
- `name: TagName` - Interned tag name
- `arguments: ArgumentList` - Interned arguments
- `span: Span` (#[no_eq]) - Position in source
- `closing_span: Option<Span>` (#[no_eq]) - End tag position if block tag
**State**: Immutable once created
**Tracked Methods**:
- `validate(db) -> Vec<ValidationError>` - Expensive validation
- `documentation(db) -> Option<String>` - Load external docs
**Regular Methods**:
- `span() -> Span` - Simple accessor
- `is_block() -> bool` - Trivial check
**Salsa**: `#[salsa::tracked]` (Salsa 0.23.0 syntax)

### SemanticVariable
**Purpose**: Represents a template variable with filters
**Fields**:
- `path: VariablePath` - Interned variable path
- `filters: FilterChain` - Interned filter chain
- `span: Span` (#[no_eq]) - Position in source
**State**: Immutable once created
**Tracked Methods**:
- `inferred_type(db) -> Option<Type>` - Complex type inference
**Regular Methods**:
- `span() -> Span` - Simple accessor
**Salsa**: `#[salsa::tracked]`

### ResolvedTemplate
**Purpose**: Template with resolved inheritance chain
**Fields**:
- `path: TemplatePath` - This template's path
- `parent: Option<ResolvedTemplate>` - Parent template if extends
- `blocks: BlockMap` - Defined blocks
- `file: File` - Source file reference
- `source_span: Span` (#[no_eq]) - Position info
**State**: Built during analysis, immutable after
**Tracked Methods**:
- `resolve_block(db, name) -> ResolvedBlock` - With cycle recovery
- `context(db) -> TemplateContext` - Expensive context building
**Inspector Integration**: Template path resolution via djls-project
**Salsa**: `#[salsa::tracked]` with cycle recovery (Salsa 0.23.0)

## Analysis Types

### AnalysisBundle
**Purpose**: Complete analysis result for a template
**Fields**:
- `semantic_elements: Vec<SemanticElement>` - All analyzed elements
- `block_tree: BlockTree` - Hierarchical block structure
- `offset_index: OffsetIndex` - Position lookup index
- `template_deps: Vec<TemplateDependency>` - External dependencies
- `construction_errors: Vec<ValidationError>` - Parse/build errors
**State**: Immutable result of analysis
**Generation**: Built by AnalysisBuilder, then interned

### SemanticElement (Enum)
**Purpose**: Discriminated union of all semantic node types
**Variants**:
- `Tag(SemanticTag)` - Template tag
- `Variable(SemanticVariable)` - Variable reference
- `Text(TextNode)` - Static text
- `Block(BlockNode)` - Block structure
**Tracked Methods** (expensive only):
- `validate(db) -> Vec<ValidationError>`
- `documentation(db) -> Option<String>`
- `inferred_type(db) -> Option<Type>`

## Type System

### Type (Enum)
**Purpose**: Python-like type representation for variables
**Variants**:
- `Any` - Unknown type
- `None` - Python None
- `String` - String type
- `Int` - Integer type
- `Float` - Float type  
- `Bool` - Boolean type
- `List(Box<Type>)` - List with element type
- `Dict(DictType)` - Dictionary type
- `Object(ObjectType)` - Django model/object
- `Union(UnionType)` - Multiple possible types

### ObjectType
**Purpose**: Known object with attributes
**Fields**:
- `name: String` - Class/type name
- `attributes: FxHashMap<String, Type>` - Known attributes
**Salsa**: `#[salsa::interned]` for deduplication

## Builder Pattern Types

### AnalysisBuilder
**Purpose**: Mutable builder for analysis results
**Fields** (not Salsa tracked):
- `semantic_nodes: Vec<SemanticNodeData>` - Building elements
- `errors: Vec<ValidationError>` - Collected errors
- `tag_names: FxHashSet<String>` - For interning
- `var_paths: FxHashSet<Vec<String>>` - For interning
**Methods**:
- `analyze(nodes, specs) -> Self` - Pure analysis logic
- `with_interning(db) -> AnalysisBundle` - Convert with interning

### InheritanceResolver
**Purpose**: Resolve template inheritance with cycle handling
**Fields**:
- `db: &dyn Db` - Database reference
- `visited: FxHashSet<TemplatePath>` - Cycle detection
**Methods**:
- `resolve(template) -> Resolution` - Core algorithm
- `build() -> ResolvedTemplate` - Final result

## Validation Rules

### Interning Constraints
- Strings must be UTF-8 valid
- Paths must not contain null bytes
- Tag names must match Django tag regex
- Variable segments must be valid Python identifiers

### Cycle Detection
- Template inheritance cycles produce empty resolution
- Block override cycles return base block
- Include cycles are reported as errors

### Memory Constraints
- Interned values are never freed during session
- Large strings (>1KB) should not be interned
- Total interning table size monitored

## State Transitions

### Analysis Pipeline
```
1. Parse (djls-templates) → NodeList
2. Analyze (AnalysisBuilder) → Inner result  
3. Intern strings (with_interning) → Interned types
4. Create tracked (Salsa) → Final AnalysisBundle
```

### Template Resolution
```
1. Load template → ResolvedTemplate (pending)
2. Find parent → Resolve parent first (recursive)
3. Detect cycle → Mark as resolved with empty parent
4. Merge blocks → ResolvedTemplate (complete)
```

### Type Inference
```
1. Check variable → Unknown type
2. Check loop scope → Iterator type if found
3. Check context → Context type if found  
4. Check parent → Parent type if found
5. Default → Type::Any
```
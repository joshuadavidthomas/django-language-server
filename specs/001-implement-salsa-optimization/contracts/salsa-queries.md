# Salsa Query Contracts

## Interning Queries

### `intern_tag_name`
**Input**: `TagName { text: String }`
**Output**: `TagName<'db>` (interned)
**Behavior**: Returns same ID for identical strings
**Performance**: O(1) after first call

### `intern_variable_path`  
**Input**: `VariablePath { segments: Vec<String> }`
**Output**: `VariablePath<'db>` (interned)
**Behavior**: Returns same ID for identical paths
**Performance**: O(1) after first call

### `intern_template_path`
**Input**: `TemplatePath { path: String }`
**Output**: `TemplatePath<'db>` (interned)
**Behavior**: Returns same ID for identical paths
**Performance**: O(1) after first call

## Analysis Queries

### `analyze_template`
**Input**: 
- `db: &dyn Db`
- `nodelist: NodeList<'db>`
**Output**: `AnalysisBundle<'db>`
**Behavior**: 
- Parses all nodes into semantic elements
- Interns all strings
- Builds offset index
- Returns cached result if unchanged
**Performance**: <100ms for 1000-line template
**Cache invalidation**: When nodelist changes

### `find_element_at_offset`
**Input**:
- `db: &dyn Db`
- `template: ResolvedTemplate<'db>`
- `offset: u32`
**Output**: `Option<TypedElement<'db>>`
**Behavior**:
- Uses offset index for O(log n) lookup
- Includes type inference for variables
- Returns None if offset out of bounds
**Performance**: <1ms
**Cache invalidation**: When template or offset changes

## Inheritance Queries

### `resolve_template`
**Input**:
- `db: &dyn Db`
- `path: TemplatePath<'db>`
**Output**: `ResolvedTemplate<'db>`
**Behavior**:
- Resolves template path via djls-project inspector
- Loads template file from resolved location
- Recursively resolves parent
- Handles cycles gracefully
- Merges block definitions
**Inspector**: Uses cached connection pool for path resolution
**Performance**: <10ms per inheritance level (excluding inspector call)
**Cycle handling**: Returns empty parent on cycle

### `resolve_block`
**Input**:
- `db: &dyn Db`  
- `template: ResolvedTemplate<'db>`
- `block_name: TagName<'db>`
**Output**: `ResolvedBlock<'db>`
**Behavior**:
- Checks local blocks first
- Traverses parent chain
- Handles super blocks
- Cycle recovery returns empty
**Performance**: <1ms
**Cycle handling**: Falls back to empty block

## Type Inference Queries

### `infer_variable_type`
**Input**:
- `db: &dyn Db`
- `template: ResolvedTemplate<'db>`
- `var_path: VariablePath<'db>`
**Output**: `Type<'db>`
**Behavior**:
- Checks loop scopes
- Checks template context
- Checks parent contexts
- Returns Type::Any if unknown
**Performance**: <5ms
**Cache invalidation**: When template or context changes

### `variables_in_scope`
**Input**:
- `db: &dyn Db`
- `template: ResolvedTemplate<'db>`
- `offset: u32`
**Output**: `FxHashMap<VariablePath<'db>, Type<'db>>`
**Behavior**:
- Collects all visible variables
- Includes loop variables at offset
- Merges parent scopes
- Respects shadowing rules
**Performance**: <10ms
**Cache invalidation**: When template changes

## Validation Queries

### `validate_template`
**Input**:
- `db: &dyn Db`
- `template: ResolvedTemplate<'db>`
**Output**: `Vec<ValidationError>`
**Behavior**:
- Validates all tags against specs
- Checks variable references
- Validates filter chains
- Checks template dependencies
**Performance**: <50ms for 1000-line template
**Cache invalidation**: When template or specs change

### `validate_tag`
**Input**:
- `db: &dyn Db`
- `tag: SemanticTag<'db>`
**Output**: `Vec<ValidationError>`
**Behavior**:
- Checks tag name validity
- Validates argument count
- Validates argument types
- Checks required/optional args
**Performance**: <1ms
**Cache invalidation**: When tag or specs change
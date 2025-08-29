# Revised Task Order for Ruff Pattern Implementation

## The Correct Architecture Understanding

Based on Ruff expert clarification:
- **SourceFile should NOT store text content** (our current implementation is wrong)
- **File content is read on-demand** through a `source_text` tracked function
- **Overlays are never Salsa inputs**, they're read through FileSystem
- **File revision triggers invalidation**, not content changes

## Implementation Order

### Phase 1: Database Foundation
1. **task-129** - Complete Database FileSystem integration
   - Database needs access to LspFileSystem to read files
   - This enables tracked functions to read through FileSystem

### Phase 2: Salsa Input Restructuring  
2. **task-126** - Bridge Salsa queries to LspFileSystem
   - Remove `text` field from SourceFile
   - Add `path` and `revision` fields
   - Create `source_text` tracked function

### Phase 3: Query Updates
3. **task-95** - Update template parsing to use source_text query
   - Update all queries to use `source_text(db, file)`
   - Remove direct text access from SourceFile

### Phase 4: LSP Integration
4. **task-112** - Add file revision tracking
   - Bump file revision when overlays change
   - This triggers Salsa invalidation

### Phase 5: Testing
5. **task-127** - Test overlay behavior and Salsa integration
   - Verify overlays work correctly
   - Test invalidation behavior

## Key Changes from Current Implementation

Current (WRONG):
```rust
#[salsa::input]
pub struct SourceFile {
    pub text: Arc<str>,  // ❌ Storing content in Salsa
}
```

Target (RIGHT):
```rust
#[salsa::input]
pub struct SourceFile {
    pub path: PathBuf,
    pub revision: u32,  // ✅ Only track changes
}

#[salsa::tracked]
pub fn source_text(db: &dyn Db, file: SourceFile) -> Arc<str> {
    // Read through FileSystem (checks overlays first)
}
```

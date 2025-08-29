# Architecture Insights from Ruff Investigation

## Key Discovery: Two-Layer Architecture

### The Problem
- LSP documents change frequently (every keystroke)
- Salsa invalidation is expensive
- Tower-lsp requires Send+Sync, but Salsa Database contains RefCell/UnsafeCell

### The Solution (Ruff Pattern)

#### Layer 1: LSP Document Management (Outside Salsa)
- Store overlays in `Session` using `Arc<DashMap<Url, TextDocument>>`  
- TextDocument contains actual content, version, language_id
- Changes are immediate, no Salsa invalidation

#### Layer 2: Salsa Incremental Computation
- Database is pure Salsa, no file storage
- Queries read through FileSystem trait
- LspFileSystem intercepts reads, returns overlay or disk content

### Critical Insights

1. **Overlays NEVER become Salsa inputs directly**
   - They're intercepted at FileSystem::read_to_string() time
   - Salsa only knows "something changed", reads content lazily

2. **StorageHandle Pattern (for tower-lsp)**
   - Session stores `StorageHandle<Database>` not Database directly
   - StorageHandle IS Send+Sync even though Database isn't
   - Create Database instances on-demand: `session.db()`

3. **File Management Location**
   - WRONG: Store files in Database (what we initially did)
   - RIGHT: Store overlays in Session, Database is pure Salsa

4. **The Bridge**
   - LspFileSystem has Arc<DashMap> to same overlays as Session
   - When Salsa queries need content, they call FileSystem
   - FileSystem checks overlays first, falls back to disk

### Implementation Flow

1. **did_open/did_change/did_close** → Update overlays in Session
2. **notify_file_changed()** → Tell Salsa something changed  
3. **Salsa query executes** → Calls FileSystem::read_to_string()
4. **LspFileSystem intercepts** → Returns overlay if exists, else disk
5. **Query gets content** → Without knowing about LSP/overlays

### Why This Works

- Fast: Overlay updates don't trigger Salsa invalidation cascade
- Thread-safe: DashMap for overlays, StorageHandle for Database
- Clean separation: LSP concerns vs computation concerns
- Efficient: Salsa caching still works, just reads through FileSystem

### Tower-lsp vs lsp-server

- **Ruff uses lsp-server**: No Send+Sync requirement, can store Database directly
- **We use tower-lsp**: Requires Send+Sync, must use StorageHandle pattern
- Both achieve same result, different mechanisms

## Critical Implementation Details (From Ruff Expert)

### The Revision Dependency Trick

**THE MOST CRITICAL INSIGHT**: In the `source_text` tracked function, calling `file.revision(db)` is what creates the Salsa dependency chain:

```rust
#[salsa::tracked]
pub fn source_text(db: &dyn Db, file: SourceFile) -> Arc<str> {
    // THIS LINE IS CRITICAL - Creates Salsa dependency on revision!
    let _ = file.revision(db);
    
    // Now read from FileSystem (checks overlays first)
    db.read_file_content(file.path(db))
}
```

Without that `file.revision(db)` call, revision changes won't trigger invalidation!

### Key Implementation Points

1. **Files have no text**: SourceFile inputs only have `path` and `revision`, never `text`
2. **Revision bumping triggers invalidation**: Change revision → source_text invalidated → dependent queries invalidated  
3. **Files created lazily**: Don't pre-create, let them be created on first access
4. **Simple counters work**: Revision can be a simple u64 counter, doesn't need timestamps
5. **StorageHandle update required**: After DB modifications in LSP handlers, must update the handle

### Common Pitfalls

- **Forgetting the revision dependency** - Without `file.revision(db)`, nothing invalidates
- **Storing text in Salsa inputs** - Breaks the entire pattern
- **Not bumping revision on overlay changes** - Queries won't see new content
- **Creating files eagerly** - Unnecessary and inefficient


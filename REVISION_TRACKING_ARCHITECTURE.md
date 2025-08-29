# Revision Tracking Architecture for Django Language Server

## Overview

This document captures the complete understanding of how to implement revision tracking for task-112, based on extensive discussions with a Ruff architecture expert. The goal is to connect the Session's overlay system with Salsa's query invalidation mechanism through per-file revision tracking.

## The Critical Breakthrough: Dual-Layer Architecture

### The Confusion We Had

We conflated two different concepts:
1. **Database struct** - The Rust struct that implements the Salsa database trait
2. **Salsa database** - The actual Salsa storage system with inputs/queries

### The Key Insight

**Database struct ≠ Salsa database**

The Database struct can contain:
- Salsa storage (the actual Salsa database)
- Additional non-Salsa data structures (like file tracking)

## The Architecture Pattern (From Ruff)

### Ruff's Implementation

```rust
// Ruff's Database contains BOTH Salsa and non-Salsa data
pub struct ProjectDatabase {
    storage: salsa::Storage<Self>,  // Salsa's data
    files: Files,                   // NOT Salsa data, but in Database struct!
}

// Files is Arc-wrapped for cheap cloning
#[derive(Clone)]
pub struct Files {
    inner: Arc<FilesInner>,  // Shared across clones
}

struct FilesInner {
    system_by_path: FxDashMap<SystemPathBuf, File>,  // Thread-safe
}
```

### Our Implementation

```rust
// Django LS Database structure
#[derive(Clone)]
pub struct Database {
    storage: salsa::Storage<Self>,
    files: Arc<DashMap<PathBuf, SourceFile>>,  // Arc makes cloning cheap!
}

// Session still uses StorageHandle for tower-lsp
pub struct Session {
    db_handle: StorageHandle<Database>,         // Still needed!
    overlays: Arc<DashMap<Url, TextDocument>>,  // LSP document state
}
```

## Why This Works with Send+Sync Requirements

1. **Arc<DashMap> is Send+Sync** - Thread-safe by design
2. **Cloning is cheap** - Only clones the Arc pointer (8 bytes)
3. **Persistence across clones** - All clones share the same DashMap
4. **StorageHandle compatible** - Database remains clonable and Send+Sync

## Implementation Details

### 1. Database Implementation

```rust
impl Database {
    pub fn get_or_create_file(&mut self, path: PathBuf) -> SourceFile {
        self.files
            .entry(path.clone())
            .or_insert_with(|| {
                // Create Salsa input with initial revision 0
                SourceFile::new(self, path, 0)
            })
            .clone()
    }
}

impl Clone for Database {
    fn clone(&self) -> self {
        Self {
            storage: self.storage.clone(),  // Salsa handles this
            files: self.files.clone(),      // Just clones Arc!
        }
    }
}
```

### 2. The Critical Pattern for Every Overlay Change

```rust
pub fn handle_overlay_change(session: &mut Session, url: Url, content: String) {
    // 1. Extract database from StorageHandle
    let mut db = session.db_handle.get();
    
    // 2. Update overlay in Session
    session.overlays.insert(url.clone(), TextDocument::new(content));
    
    // 3. Get or create file in Database
    let path = path_from_url(&url);
    let file = db.get_or_create_file(path);
    
    // 4. Bump revision (simple incrementing counter)
    let current_rev = file.revision(&db);
    file.set_revision(&mut db).to(current_rev + 1);
    
    // 5. Update StorageHandle with modified database
    session.db_handle.update(db);  // CRITICAL!
}
```

### 3. LSP Handler Updates

#### did_open

```rust
pub fn did_open(&mut self, params: DidOpenTextDocumentParams) {
    let mut db = self.session.db_handle.get();
    
    // Set overlay
    self.session.overlays.insert(
        params.text_document.uri.clone(),
        TextDocument::new(params.text_document.text)
    );
    
    // Create file with initial revision 0
    let path = path_from_url(&params.text_document.uri);
    db.get_or_create_file(path);  // Creates with revision 0
    
    self.session.db_handle.update(db);
}
```

#### did_change

```rust
pub fn did_change(&mut self, params: DidChangeTextDocumentParams) {
    let mut db = self.session.db_handle.get();
    
    // Update overlay
    let new_content = params.content_changes[0].text.clone();
    self.session.overlays.insert(
        params.text_document.uri.clone(),
        TextDocument::new(new_content)
    );
    
    // Bump revision
    let path = path_from_url(&params.text_document.uri);
    let file = db.get_or_create_file(path);
    let new_rev = file.revision(&db) + 1;
    file.set_revision(&mut db).to(new_rev);
    
    self.session.db_handle.update(db);
}
```

#### did_close

```rust
pub fn did_close(&mut self, params: DidCloseTextDocumentParams) {
    let mut db = self.session.db_handle.get();
    
    // Remove overlay
    self.session.overlays.remove(&params.text_document.uri);
    
    // Bump revision to trigger re-read from disk
    let path = path_from_url(&params.text_document.uri);
    if let Some(file) = db.files.get(&path) {
        let new_rev = file.revision(&db) + 1;
        file.set_revision(&mut db).to(new_rev);
    }
    
    self.session.db_handle.update(db);
}
```

## Key Implementation Guidelines from Ruff Expert

### 1. File Tracking Location

- Store in Database struct (not Session)
- Use Arc<DashMap> for thread-safety and cheap cloning
- This keeps file tracking close to where it's used

### 2. Revision Management

- Use simple incrementing counter per file (not timestamps)
- Each file has independent revision tracking
- Revision just needs to change, doesn't need to be monotonic
- Example: `file.set_revision(&mut db).to(current + 1)`

### 3. Lazy File Creation

Files should be created:
- On did_open (via get_or_create_file)
- On first query access if needed
- NOT eagerly for all possible files

### 4. File Lifecycle

- **On open**: Create file with revision 0
- **On change**: Bump revision to trigger invalidation
- **On close**: Keep file alive, bump revision for re-read from disk
- **Never remove**: Files stay in tracking even after close

### 5. Batch Changes for Performance

When possible, batch multiple changes:

```rust
pub fn apply_batch_changes(&mut self, changes: Vec<FileChange>) {
    let mut db = self.session.db_handle.get();
    
    for change in changes {
        // Process each change
        let file = db.get_or_create_file(change.path);
        file.set_revision(&mut db).to(file.revision(&db) + 1);
    }
    
    // Single StorageHandle update at the end
    self.session.db_handle.update(db);
}
```

### 6. Thread Safety with DashMap

Use DashMap's atomic entry API:

```rust
self.files.entry(path.clone())
    .and_modify(|file| {
        // Modify existing
        file.set_revision(db).to(new_rev);
    })
    .or_insert_with(|| {
        // Create new
        SourceFile::builder(path)
            .revision(0)
            .new(db)
    });
```

## Critical Pitfalls to Avoid

1. **NOT BUMPING REVISION** - Every overlay change MUST bump revision or Salsa won't invalidate
2. **FORGETTING STORAGEHANDLE UPDATE** - Must call `session.db_handle.update(db)` after changes
3. **CREATING FILES EAGERLY** - Let files be created lazily on first access
4. **USING TIMESTAMPS** - Simple incrementing counter is sufficient
5. **REMOVING FILES** - Keep files alive even after close, just bump revision

## The Two-Layer Model

### Layer 1: Non-Salsa (but in Database struct)
- `Arc<DashMap<PathBuf, SourceFile>>` - File tracking
- Thread-safe via Arc+DashMap
- Cheap to clone via Arc
- Acts as a lookup table

### Layer 2: Salsa Inputs
- `SourceFile` entities created via `SourceFile::new(db)`
- Have revision fields for invalidation
- Tracked by Salsa's dependency system
- Invalidation cascades through dependent queries

## Complete Architecture Summary

| Component | Contains | Purpose |
|-----------|----------|---------|
| **Database** | `storage` + `Arc<DashMap<PathBuf, SourceFile>>` | Salsa queries + file tracking |
| **Session** | `StorageHandle<Database>` + `Arc<DashMap<Url, TextDocument>>` | LSP state + overlays |
| **StorageHandle** | `Arc<ArcSwap<Option<Database>>>` | Bridge for tower-lsp lifetime requirements |
| **SourceFile** | Salsa input with path + revision | Triggers query invalidation |

## The Flow

1. **LSP request arrives** → tower-lsp handler
2. **Extract database** → `db = session.db_handle.get()`
3. **Update overlay** → `session.overlays.insert(url, content)`
4. **Get/create file** → `db.get_or_create_file(path)`
5. **Bump revision** → `file.set_revision(&mut db).to(current + 1)`
6. **Update handle** → `session.db_handle.update(db)`
7. **Salsa invalidates** → `source_text` query re-executes
8. **Queries see new content** → Through overlay-aware FileSystem

## Why StorageHandle is Still Essential

1. **tower-lsp requirement**: Needs 'static lifetime for async handlers
2. **Snapshot management**: Safe extraction and update of database
3. **Thread safety**: Bridges async boundaries safely
4. **Atomic updates**: Ensures consistent state transitions

## Testing Strategy

1. **Revision bumping**: Verify each overlay operation bumps revision
2. **Invalidation cascade**: Ensure source_text re-executes after revision bump
3. **Thread safety**: Concurrent overlay updates work correctly
4. **Clone behavior**: Database clones share the same file tracking
5. **Lazy creation**: Files only created when accessed

## Implementation Checklist

- [ ] Add `Arc<DashMap<PathBuf, SourceFile>>` to Database struct
- [ ] Implement Clone for Database (clone both storage and Arc)
- [ ] Create `get_or_create_file` method using atomic entry API
- [ ] Update did_open to create files with revision 0
- [ ] Update did_change to bump revision after overlay update
- [ ] Update did_close to bump revision (keep file alive)
- [ ] Ensure StorageHandle updates after all database modifications
- [ ] Add tests for revision tracking and invalidation

## Questions That Were Answered

1. **Q: Files in Database or Session?**
   A: In Database, but Arc-wrapped for cheap cloning

2. **Q: How does this work with Send+Sync?**
   A: Arc<DashMap> is Send+Sync, making Database clonable and thread-safe

3. **Q: Do we still need StorageHandle?**
   A: YES! It bridges tower-lsp's lifetime requirements

4. **Q: Timestamp or counter for revisions?**
   A: Simple incrementing counter per file

5. **Q: Remove files on close?**
   A: No, keep them alive and bump revision for re-read

## The Key Insight

Database struct is a container that holds BOTH:
- Salsa storage (for queries and inputs)
- Non-Salsa data (file tracking via Arc<DashMap>)

Arc makes the non-Salsa data cheap to clone while maintaining Send+Sync compatibility. This is the pattern Ruff uses and what we should implement.
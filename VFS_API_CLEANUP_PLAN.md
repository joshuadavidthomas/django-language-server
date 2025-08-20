# VFS API Cleanup Plan for Django Language Server

## Context
This VFS is for a **Language Server Protocol (LSP) implementation**, not a general filesystem or text editor. Key LSP semantics:
- **Editor owns files**: VS Code/Neovim manages actual file operations
- **We track working state**: Memory layer = unsaved edits from editor
- **Physical layer is read-only**: We NEVER modify disk files
- **We MUST implement the vfs::FileSystem trait**

## The Two-Layer Mental Model

The VFS two-layer design perfectly matches LSP needs:

### Write Operations → Memory Layer Only
All mutations happen in memory (we never touch disk):
- `create_file()` → Creates new unsaved file in memory
- `create_dir()` → Creates directory structure in memory
- `append_file()` → Appends to memory version
- `remove_file()` → Removes from memory (file closed in editor)
- `remove_dir()` → Removes from memory

### Read Operations → Overlay (Memory First, Physical Fallback)
All queries check memory first, then fall back to physical:
- `open_file()` → Returns unsaved edits if exist, else disk version
- `read_dir()` → Merges memory and physical entries
- `exists()` → Checks if file exists in memory OR on disk
- `metadata()` → Returns metadata from memory if exists, else physical

This maps perfectly to LSP's three file states:
1. **Unopened files**: Only in physical layer (on disk)
2. **Opened files with unsaved changes**: In memory layer (overlay)
3. **New unsaved files**: Only in memory layer (no disk file yet)

## Current Problems (Identified by Experts)

### Barbara Liskov (Abstraction Expert)
- **Mixed abstraction levels**: `write_memory()` exposes implementation detail in name
- **Broken information hiding**: `memory_root()`, `physical_root()` expose internals
- **Inconsistent interfaces**: Trait methods vs inherent methods have different semantics

### Rich Hickey (Simplicity Expert)  
- **Complected concerns**: Methods mix "what" (file operations) with "how" (memory layer)
- **Accidental complexity**: Users forced to understand two-layer implementation
- **Not simple**: Method names like `write_memory()` expose implementation not intent

### Alan Kay (OOP Expert)
- **Poor encapsulation**: Internal structure exposed through `memory_root()`, `physical_root()`
- **Inconsistent message interface**: No uniform behavior pattern across methods
- **Procedural thinking**: Methods named after implementation not behavior

## The Solution

### Core Insight
We need TWO levels of API:
1. **vfs::FileSystem trait** (required): Low-level file operations with file handles
2. **Inherent methods** (our convenience): High-level string operations for LSP

### Current Structure in `crates/djls-server/src/workspace/fs.rs`

```rust
// TRAIT METHODS (from vfs::FileSystem) - Keep these as-is
impl vfs::FileSystem for FileSystem {
    fn read_dir(&self, path: &str) -> VfsResult<Box<dyn Iterator<Item = String> + Send>>
    fn create_dir(&self, path: &str) -> VfsResult<()>
    fn open_file(&self, path: &str) -> VfsResult<Box<dyn SeekAndRead + Send>>
    fn create_file(&self, path: &str) -> VfsResult<Box<dyn SeekAndWrite + Send>>
    fn append_file(&self, path: &str) -> VfsResult<Box<dyn SeekAndWrite + Send>>
    fn metadata(&self, path: &str) -> VfsResult<VfsMetadata>
    fn exists(&self, path: &str) -> VfsResult<bool>
    fn remove_file(&self, path: &str) -> VfsResult<()>
    fn remove_dir(&self, path: &str) -> VfsResult<()>
    // ... time setters ...
}

// INHERENT METHODS (our additions) - These need cleanup
impl FileSystem {
    pub fn read(&self, path: &str) -> VfsResult<String>  // Good, but rename
    pub fn write_memory(&self, path: &str, content: &str) -> VfsResult<()>  // BAD NAME
    pub fn clear_memory(&self, path: &str) -> VfsResult<()>  // BAD NAME
    pub fn exists(&self, path: &str) -> VfsResult<bool>  // Duplicates trait
    pub fn memory_root(&self) -> VfsPath  // EXPOSES INTERNALS
    pub fn physical_root(&self) -> VfsPath  // EXPOSES INTERNALS
    pub fn root(&self) -> VfsPath  // EXPOSES INTERNALS
}
```

## Implementation Plan

### Step 1: Rename Inherent Methods
```rust
impl FileSystem {
    /// High-level convenience: Read file as string (memory first, then physical)
    pub fn read_to_string(&self, path: &str) -> VfsResult<String>  // was read()
    
    /// High-level convenience: Write string to memory layer (unsaved changes)
    pub fn write_string(&self, path: &str, content: &str) -> VfsResult<()>  // was write_memory()
    
    /// High-level convenience: Discard unsaved changes (clear from memory)
    pub fn discard_changes(&self, path: &str) -> VfsResult<()>  // was clear_memory()
    
    // Keep exists() as it wraps the trait method
}
```

### Step 2: Remove Implementation-Exposing Methods
Delete these entirely:
- `memory_root()`
- `physical_root()` 
- `root()`

### Step 3: Document LSP Semantics
Add clear documentation explaining:
- Memory layer = unsaved edits from editor
- Physical layer = read-only disk state
- All writes go to memory (editor owns disk writes)

### Step 4: Update Call Sites
- `vfs.write_memory(path, content)` → `vfs.write_string(path, content)`
- `vfs.clear_memory(path)` → `vfs.discard_changes(path)`
- `vfs.read(path)` → `vfs.read_to_string(path)`
- Remove any usage of `memory_root()`, `physical_root()`, `root()`

## Why This Design is Correct for LSP

### Memory-Only Operations are Intentional
- **write_string()**: Editor sends didChange, we track unsaved content
- **discard_changes()**: Editor sends didClose, we clear our cache
- **We never write to disk**: That's the editor's job via didSave

### Two-Layer Design is Perfect for LSP
- Memory layer = working state (unsaved edits)
- Physical layer = baseline (saved state on disk)
- Overlay semantics match LSP needs exactly

## Testing Considerations
- Existing tests should mostly work with renamed methods
- Remove tests that directly access `memory_root()` or `physical_root()`
- Add tests for LSP semantics (unsaved changes, discard, etc.)

## Summary
The VFS design is actually correct for LSP - the problem was:
1. Method names exposed implementation (`write_memory` vs `write_string`)
2. Unnecessary methods exposed internals (`memory_root`, etc.)
3. Missing documentation about LSP-specific semantics

The fix is simple: rename methods to describe intent not implementation, remove internal-exposing methods, and document the LSP context clearly.
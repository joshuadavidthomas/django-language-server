# OUTDATED - See ARCHITECTURE_INSIGHTS.md for current solution

## This document is preserved for historical context but is OUTDATED
## We found the StorageHandle solution that solves the Send+Sync issue

# Key Findings from Ruff's Architecture

Based on the exploration, here's what we discovered:

## Current Django LS Architecture

### What We Have:
1. `Database` struct with `#[derive(Clone)]` and Salsa storage
2. `WorkspaceDatabase` that wraps `Database` and uses `DashMap` for thread-safe file storage
3. `Session` that owns `WorkspaceDatabase` directly (not wrapped in Arc<Mutex>)
4. Tower-LSP server that requires `Send + Sync` for async handlers

### The Problem:
- `Database` is not `Sync` due to `RefCell<QueryStack>` and `UnsafeCell<HashMap>` in Salsa's `ZalsaLocal`
- This prevents `Session` from being `Sync`, which breaks tower-lsp async handlers

## Ruff's Solution (From Analysis)

### They Don't Make Database Sync!
The key insight is that Ruff **doesn't actually make the database Send + Sync**. Instead:

1. **Clone for Background Work**: They clone the database for each background task
2. **Move Not Share**: The cloned database is *moved* into the task (requires Send, not Sync)
3. **Message Passing**: Results are sent back via channels

### Critical Difference:
- Ruff uses a custom LSP implementation that doesn't require `Sync` on the session
- Tower-LSP *does* require `Sync` because handlers take `&self`

## The Real Problem

Tower-LSP's `LanguageServer` trait requires:
```rust
async fn initialize(&self, ...) -> ... 
//                  ^^^^^ This requires self to be Sync!
```

But with Salsa's current implementation, the Database can never be Sync.

## Solution Options

### Option 1: Wrap Database in Arc<Mutex> (Current Workaround)
```rust
pub struct Session {
    database: Arc<Mutex<WorkspaceDatabase>>,
    // ...
}
```
Downsides: Lock contention, defeats purpose of Salsa's internal optimization

### Option 2: Move Database Out of Session
```rust
pub struct Session {
    // Don't store database here
    file_index: Arc<DashMap<Url, FileContent>>,
    settings: Settings,
}

// Create database on demand for each request
impl LanguageServer for Server {
    async fn some_handler(&self) {
        let db = create_database_from_index(&self.session.file_index);
        // Use db for this request
    }
}
```

### Option 3: Use Actor Pattern
```rust
pub struct DatabaseActor {
    database: WorkspaceDatabase,
    rx: mpsc::Receiver<DatabaseCommand>,
}

pub struct Session {
    db_tx: mpsc::Sender<DatabaseCommand>,
}
```

### Option 4: Custom unsafe Send/Sync implementation
This is risky but possible if we ensure single-threaded access patterns.

## The Salsa Version Mystery

We're using the exact same Salsa commit as Ruff, with the same features. The issue is NOT the Salsa version, but how tower-lsp forces us to use it.

Ruff likely either:
1. Doesn't use tower-lsp (has custom implementation)
2. Or structures their server differently to avoid needing Sync on the database

# OUTDATED - See ARCHITECTURE_INSIGHTS.md for current solution

## This document is preserved for historical context but is OUTDATED
## We found the StorageHandle solution that solves the Send+Sync issue

# Critical Discovery: The Tower-LSP vs lsp-server Architectural Mismatch

## The Real Problem

Your Ruff expert friend is correct. The fundamental issue is:

### What We Found:

1. **Salsa commit a3ffa22 uses `RefCell` and `UnsafeCell`** - These are inherently not `Sync`
2. **Tower-LSP requires `Sync`** - Because handlers take `&self` in async contexts
3. **Ruff uses `lsp-server`** - Which doesn't require `Sync` on the server struct

### The Mystery:

Your expert suggests Ruff's database IS `Send + Sync`, but our testing shows that with the same Salsa commit, the database contains:
- `RefCell<salsa::active_query::QueryStack>` (not Sync)
- `UnsafeCell<HashMap<IngredientIndex, PageIndex>>` (not Sync)

## Possible Explanations:

### Theory 1: Ruff Has Custom Patches
Ruff might have additional patches or workarounds not visible in the commit hash.

### Theory 2: Different Usage Pattern
Ruff might structure their database differently to avoid the Sync requirement entirely.

### Theory 3: lsp-server Architecture
Since Ruff uses `lsp-server` (not `tower-lsp`), they might never need the database to be Sync:
- They clone the database for background work (requires Send only)
- The main thread owns the database, background threads get clones
- No shared references across threads

## Verification Needed:

1. **Check if Ruff's database is actually Sync**:
   - Look for unsafe impl Sync in their codebase
   - Check if they wrap the database differently

2. **Understand lsp-server's threading model**:
   - How does it handle async without requiring Sync?
   - What's the message passing pattern?

## Solution Decision Matrix (Updated):

| Solution | Effort | Performance | Risk | Compatibility |
|----------|---------|------------|------|---------------|
| **Switch to lsp-server** | High | High | Medium | Perfect Ruff parity |
| **Actor Pattern** | Medium | Medium | Low | Works with tower-lsp |
| **Arc<Mutex>** | Low | Poor | Low | Works but slow |
| **Unsafe Sync wrapper** | Low | High | Very High | Dangerous |
| **Database per request** | Medium | Poor | Low | Loses memoization |

## Recommended Action Plan:

### Immediate (Today):
1. Verify that Salsa a3ffa22 truly has RefCell/UnsafeCell
2. Check if there are any Ruff-specific patches to Salsa
3. Test the actor pattern as a better alternative to Arc<Mutex>

### Short-term (This Week):
1. Implement actor pattern if Salsa can't be made Sync
2. OR investigate unsafe Sync wrapper with careful single-threaded access guarantees

### Long-term (This Month):
1. Consider migrating to lsp-server for full Ruff compatibility
2. OR contribute Sync support to Salsa upstream

## The Key Insight:

**Tower-LSP's architecture is fundamentally incompatible with Salsa's current design.**

Ruff avoided this by using `lsp-server`, which has a different threading model that doesn't require Sync on the database.

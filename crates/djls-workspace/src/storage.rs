use salsa::StorageHandle;

use crate::db::Database;

/// Safe wrapper for [`StorageHandle`](salsa::StorageHandle) that prevents misuse through type safety.
///
/// This enum ensures that database handles can only be in one of two valid states,
/// making invalid states unrepresentable and eliminating the need for placeholder
/// handles during mutations.
///
/// ## Panic Behavior
///
/// Methods in this type may panic when the state machine invariants are violated.
/// These panics represent **programming bugs**, not runtime errors that should be
/// handled. They indicate violations of the internal API contract, similar to how
/// `RefCell::borrow_mut()` panics on double borrows. The panics ensure that bugs
/// are caught during development rather than causing silent data corruption.
pub enum SafeStorageHandle {
    /// Handle is available for use
    Available(StorageHandle<Database>),
    /// Handle has been taken for mutation - no handle available
    TakenForMutation,
}

impl SafeStorageHandle {
    /// Create a new `SafeStorageHandle` in the `Available` state
    pub fn new(handle: StorageHandle<Database>) -> Self {
        Self::Available(handle)
    }

    /// Take the handle for mutation, leaving the enum in `TakenForMutation` state.
    ///
    /// ## Panics
    ///
    /// Panics if the handle has already been taken for mutation.
    pub fn take_for_mutation(&mut self) -> StorageHandle<Database> {
        match std::mem::replace(self, Self::TakenForMutation) {
            Self::Available(handle) => handle,
            Self::TakenForMutation => panic!(
                "Database handle already taken for mutation. This indicates a programming error - \
                 ensure you're not calling multiple mutation operations concurrently or forgetting \
                 to restore the handle after a previous mutation."
            ),
        }
    }

    /// Restore the handle after mutation, returning it to `Available` state.
    ///
    /// ## Panics
    ///
    /// Panics if the handle is not currently taken for mutation.
    pub fn restore_from_mutation(&mut self, handle: StorageHandle<Database>) {
        match self {
            Self::TakenForMutation => {
                *self = Self::Available(handle);
            }
            Self::Available(_) => panic!(
                "Cannot restore database handle - handle is not currently taken for mutation. \
                 This indicates a programming error in the StorageHandleGuard implementation."
            ),
        }
    }

    /// Get a clone of the handle for read-only operations.
    ///
    /// ## Panics
    ///
    /// Panics if the handle is currently taken for mutation.
    pub fn clone_for_read(&self) -> StorageHandle<Database> {
        match self {
            Self::Available(handle) => handle.clone(),
            Self::TakenForMutation => panic!(
                "Cannot access database handle for read - handle is currently taken for mutation. \
                 Wait for the current mutation operation to complete."
            ),
        }
    }

    /// Take the handle for mutation with automatic restoration via guard.
    /// This ensures the handle is always restored even if the operation panics.
    pub fn take_guarded(&mut self) -> StorageHandleGuard {
        StorageHandleGuard::new(self)
    }
}

/// State of the [`StorageHandleGuard`] during its lifetime.
///
/// See [`StorageHandleGuard`] for usage and state machine details.
enum GuardState {
    /// Guard holds the handle, ready to be consumed
    Active { handle: StorageHandle<Database> },
    /// Handle consumed, awaiting restoration
    Consumed,
    /// Handle restored to [`SafeStorageHandle`]
    Restored,
}

/// RAII guard for safe [`StorageHandle`](salsa::StorageHandle) management during mutations.
///
/// This guard ensures that database handles are automatically restored even if
/// panics occur during mutation operations. It prevents double-takes and
/// provides clear error messages for misuse.
///
/// ## State Machine
///
/// The guard follows these valid state transitions:
/// - `Active` → `Consumed` (via `handle()` method)
/// - `Consumed` → `Restored` (via `restore()` method)
///
/// ## Invalid Transitions
///
/// Invalid operations will panic with specific error messages:
/// - `handle()` on `Consumed` state: "[`StorageHandle`](salsa::StorageHandle) already consumed"
/// - `handle()` on `Restored` state: "Cannot consume handle - guard has already been restored"
/// - `restore()` on `Active` state: "Cannot restore handle - it hasn't been consumed yet"
/// - `restore()` on `Restored` state: "Handle has already been restored"
///
/// ## Drop Behavior
///
/// The guard will panic on drop unless it's in the `Restored` state:
/// - Drop in `Active` state: "`StorageHandleGuard` dropped without using the handle"
/// - Drop in `Consumed` state: "`StorageHandleGuard` dropped without restoring handle"
/// - Drop in `Restored` state: No panic - proper cleanup completed
///
/// ## Usage Example
///
/// ```rust,ignore
/// let mut guard = StorageHandleGuard::new(&mut safe_handle);
/// let handle = guard.handle();           // Active → Consumed
/// // ... perform mutations with handle ...
/// guard.restore(updated_handle);         // Consumed → Restored
/// // Guard drops cleanly in Restored state
/// ```
#[must_use = "StorageHandleGuard must be used - dropping it immediately defeats the purpose"]
pub struct StorageHandleGuard<'a> {
    /// Reference to the workspace's `SafeStorageHandle` for restoration
    safe_handle: &'a mut SafeStorageHandle,
    /// Current state of the guard and handle
    state: GuardState,
}

impl<'a> StorageHandleGuard<'a> {
    /// Create a new guard by taking the handle from the `SafeStorageHandle`.
    pub fn new(safe_handle: &'a mut SafeStorageHandle) -> Self {
        let handle = safe_handle.take_for_mutation();
        Self {
            safe_handle,
            state: GuardState::Active { handle },
        }
    }

    /// Get the [`StorageHandle`](salsa::StorageHandle) for mutation operations.
    ///
    /// ## Panics
    ///
    /// Panics if the handle has already been consumed or restored.
    pub fn handle(&mut self) -> StorageHandle<Database> {
        match std::mem::replace(&mut self.state, GuardState::Consumed) {
            GuardState::Active { handle } => handle,
            GuardState::Consumed => panic!(
                "StorageHandle already consumed from guard. Each guard can only provide \
                 the handle once - this prevents accidental multiple uses."
            ),
            GuardState::Restored => panic!(
                "Cannot consume handle - guard has already been restored. Once restored, \
                 the guard cannot provide the handle again."
            ),
        }
    }

    /// Restore the handle manually before the guard drops.
    ///
    /// This is useful when you want to restore the handle and continue using
    /// the workspace in the same scope.
    ///
    /// ## Panics
    ///
    /// Panics if the handle hasn't been consumed yet, or if already restored.
    pub fn restore(mut self, handle: StorageHandle<Database>) {
        match self.state {
            GuardState::Consumed => {
                self.safe_handle.restore_from_mutation(handle);
                self.state = GuardState::Restored;
            }
            GuardState::Active { .. } => panic!(
                "Cannot restore handle - it hasn't been consumed yet. Call guard.handle() \
                 first to get the handle, then restore the updated handle after mutations."
            ),
            GuardState::Restored => {
                panic!("Handle has already been restored. Each guard can only restore once.")
            }
        }
    }
}

impl Drop for StorageHandleGuard<'_> {
    fn drop(&mut self) {
        // Provide specific error messages based on the exact state
        // Avoid double-panic during unwinding
        if !std::thread::panicking() {
            match &self.state {
                GuardState::Active { .. } => {
                    panic!(
                        "StorageHandleGuard dropped without using the handle. Either call \
                         guard.handle() to consume the handle for mutations, or ensure the \
                         guard is properly used in your mutation workflow."
                    );
                }
                GuardState::Consumed => {
                    panic!(
                        "StorageHandleGuard dropped without restoring handle. You must call \
                         guard.restore(updated_handle) to properly restore the database handle \
                         after mutation operations complete."
                    );
                }
                GuardState::Restored => {
                    // All good - proper cleanup completed
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use dashmap::DashMap;

    use super::*;
    use crate::buffers::Buffers;
    use crate::fs::OsFileSystem;
    use crate::fs::WorkspaceFileSystem;

    fn create_test_handle() -> StorageHandle<Database> {
        Database::new(
            Arc::new(WorkspaceFileSystem::new(
                Buffers::new(),
                Arc::new(OsFileSystem),
            )),
            Arc::new(DashMap::new()),
        )
        .storage()
        .clone()
        .into_zalsa_handle()
    }

    #[test]
    fn test_handle_lifecycle() {
        // Test the happy path: take handle, use it, restore it
        let mut safe_handle = SafeStorageHandle::new(create_test_handle());

        let handle = safe_handle.take_for_mutation();

        // Simulate using the handle to create a database
        let storage = handle.into_storage();
        let db = Database::from_storage(
            storage,
            Arc::new(WorkspaceFileSystem::new(
                Buffers::new(),
                Arc::new(OsFileSystem),
            )),
            Arc::new(DashMap::new()),
        );

        // Get new handle after simulated mutation
        let new_handle = db.storage().clone().into_zalsa_handle();

        safe_handle.restore_from_mutation(new_handle);

        // Should be able to take it again
        let _handle2 = safe_handle.take_for_mutation();
    }

    #[test]
    fn test_guard_auto_restore_on_drop() {
        let mut safe_handle = SafeStorageHandle::new(create_test_handle());

        {
            let mut guard = safe_handle.take_guarded();
            let handle = guard.handle();

            // Simulate mutation
            let storage = handle.into_storage();
            let db = Database::from_storage(
                storage,
                Arc::new(WorkspaceFileSystem::new(
                    Buffers::new(),
                    Arc::new(OsFileSystem),
                )),
                Arc::new(DashMap::new()),
            );
            let new_handle = db.storage().clone().into_zalsa_handle();

            guard.restore(new_handle);
        } // Guard drops here in Restored state - should be clean

        // Should be able to use handle again after guard drops
        let _handle = safe_handle.clone_for_read();
    }

    #[test]
    #[should_panic(expected = "Database handle already taken for mutation")]
    fn test_panic_on_double_mutation() {
        let mut safe_handle = SafeStorageHandle::new(create_test_handle());

        let _handle1 = safe_handle.take_for_mutation();
        // Can't take handle twice, should panic
        let _handle2 = safe_handle.take_for_mutation();
    }

    #[test]
    #[should_panic(expected = "Cannot access database handle for read")]
    fn test_panic_on_read_during_mutation() {
        let mut safe_handle = SafeStorageHandle::new(create_test_handle());

        let _handle = safe_handle.take_for_mutation();
        // Can't read while mutating, should panic
        let _read = safe_handle.clone_for_read();
    }

    #[test]
    #[should_panic(expected = "Cannot restore handle - it hasn't been consumed yet")]
    fn test_guard_enforces_consume_before_restore() {
        let mut safe_handle = SafeStorageHandle::new(create_test_handle());
        let guard = safe_handle.take_guarded();

        let dummy_handle = create_test_handle();
        // Try to restore without consuming, should panic
        guard.restore(dummy_handle);
    }

    #[test]
    #[should_panic(expected = "StorageHandleGuard dropped without restoring handle")]
    fn test_guard_panics_if_dropped_without_restore() {
        let mut safe_handle = SafeStorageHandle::new(create_test_handle());

        {
            let mut guard = safe_handle.take_guarded();
            let _handle = guard.handle();
        } // Explicitly drop guard without restore, should panic
    }
}

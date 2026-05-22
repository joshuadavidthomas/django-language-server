//! Infrastructure boundary for workspace and installed-app scanning.
//!
//! Phase 8 keeps static Python module inventory in `djls-project`; concrete
//! database code should put imperative filesystem scanning helpers here instead
//! of growing `db.rs` or rebuilding a monolithic refresh pipeline.

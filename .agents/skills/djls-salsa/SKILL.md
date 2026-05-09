---
name: djls-salsa
description: Use when editing Salsa inputs, tracked functions, database traits, setters, SemanticDb, crate::Db, invalidation behavior, or incremental computation in django-language-server. Handles project-specific Salsa conventions and required impl updates.
---

# DJLS Salsa

Use this before changing Salsa inputs, tracked functions, database traits, or query plumbing.

## Setter and invalidation rules

- Setter API is `project.set_field(db).to(value)`, not `.set_field(db, value)`.
- Compare before setting because setters always invalidate.
- Use `#[returns(ref)]` on fields returning owned types.
- Salsa returns `&T` for `#[returns(ref)]`, so compare with `&new_value`.
- Tracked return types need `PartialEq` for backdate optimization.

## Required updates

- When adding `SemanticDb` methods, update impls in:
  - `djls-db/src/db.rs`
  - `djls-bench/src/db.rs`
- When adding `crate::Db` methods in `djls-semantic`, update all test database impls. Missing impls produce E0046.

## Checks

Search with `rg`, not `grep`:

```bash
rg "impl crate::Db" crates/djls-semantic/ -g '*.rs'
rg "trait SemanticDb|impl SemanticDb" crates/ -g '*.rs'
```

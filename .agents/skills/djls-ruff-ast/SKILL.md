---
name: djls-ruff-ast
description: Use when editing djls-extraction or Rust code that consumes Ruff Python AST APIs, including parse_module, parameters, boxed expressions, f-strings, or exception handlers. Handles known Ruff AST shape gotchas in django-language-server.
---

# DJLS Ruff AST

Use this before editing `djls-extraction` or code that consumes Ruff Python AST nodes.

## Known API shapes

- Parse modules with `ruff_python_parser::parse_module(source)` and call `.into_syntax()` for `ModModule` AST.
- Function defaults are per-parameter. There is no top-level `defaults` field.
  - Use `ParameterWithDefault { parameter, default: Option<Box<Expr>> }`.
- `StmtWhile.test`, `StmtIf.test`, and similar fields are `Box<Expr>`.
  - Dereference with `&*` for pattern matching.
- `FStringValue` uses `.iter()`, not `.parts()`, for `FStringPart` iteration.
- `ExceptHandler::ExceptHandler` is irrefutable.
  - Use `let`, not `if let`.

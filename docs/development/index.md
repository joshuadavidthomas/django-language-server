---
title: Overview
---
# Development

The project is written in Rust using PyO3 for Python integration. Here is a high-level overview of the project and the various crates:

- Main CLI interface ([`crates/djls/`](./crates/djls/))
- Django and Python project introspection ([`crates/djls-project/`](./crates/djls-project/))
- LSP server implementation ([`crates/djls-server/`](./crates/djls-server/))
- Template parsing ([`crates/djls-templates/`](./crates/djls-templates/))
- Tokio-based background task management ([`crates/djls-worker/`](./crates/djls-worker/))

Code contributions are welcome from developers of all backgrounds. Rust expertise is valuable for the LSP server and core components, but Python and Django developers should not be deterred by the Rust codebase - Django expertise is just as valuable. Understanding Django's internals and common development patterns helps inform what features would be most valuable.

So far it's all been built by a [a simple country CRUD web developer](https://youtu.be/7ij_1SQqbVo?si=hwwPyBjmaOGnvPPI&t=53) learning Rust along the way - send help!

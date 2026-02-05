---
date: 2026-02-04
repo: https://github.com/joshuadavidthomas/django-language-server
branch: main
commit: 4d18e6e
query: "What options are there for parsing Python source code to an AST in Rust?"
tags: [rust, python, parser, ast, lsp, ruff, rustpython, tree-sitter]
---

# Python AST Parsing Options in Rust

## Executive Summary

There are four main options for parsing Python to AST in Rust:

| Option                 | Output | Last Commit | Stars | Maintained       |
| ---------------------- | ------ | ----------- | ----- | ---------------- |
| **ruff_python_parser** | AST    | 2026-02-04  | 45.6k | ✅ Very active   |
| **rustpython-parser**  | AST    | 2025-08-20  | 113   | ⚠️ Stale         |
| **python-ast**         | AST    | 2025-08-06  | 12    | ❌ Abandoned     |
| ~~tree-sitter-python~~ | CST    | 2025-09-15  | 520   | N/A - wrong tool |

**Key Finding:** rustpython-parser hasn't been updated in 18 months and its README says it's "superseded" by Ruff's parser. Meanwhile, Ruff's parser has daily commits and powers all of Astral's Python tooling.

**Recommendation for djls:** Use **ruff_python_parser** via git with SHA pinning:

```toml
ruff_python_parser = { git = "https://github.com/astral-sh/ruff", rev = "<sha>" }
ruff_python_ast = { git = "https://github.com/astral-sh/ruff", rev = "<sha>" }
```

- Only actively maintained Python **AST** parser in Rust
- Modern Python syntax support (3.12+, 3.13+)
- Battle-tested in Ruff and ty
- Pin to SHA for stability, update when ready

Note: tree-sitter-python produces CST (concrete syntax tree), not AST - not suitable for precise AST parsing needs.

---

## 1. Ruff's Python Parser (`ruff_python_parser`)

### Overview

Ruff has an extremely fast, hand-written recursive descent parser. As of v0.4.0, it replaced their LALRPOP-based parser with a 2x faster implementation.

### Key Findings

- **NOT published to crates.io** - `ruff_python_parser` and `ruff_python_ast` crates don't exist on crates.io
- Internal crates within the astral-sh/ruff monorepo
- To use, you'd need a git dependency:

    ```toml
    ruff_python_parser = { git = "https://github.com/astral-sh/ruff", branch = "main" }
    ```

- No stability guarantees for external use
- Very fast - blog post claims 2x faster than their previous LALRPOP parser
- Powers Ruff linting/formatting and Astral's new `ty` type checker

### Maintenance Activity

- **GitHub Stars:** 45,578
- **Last Commit:** 2026-02-04 (today - multiple commits daily)
- **Open Issues:** 1,889
- **Activity Level:** Extremely high - Astral has a full team working on Ruff and ty

### Viability Assessment: **✅ Recommended**

- By far the most actively maintained Python parser in Rust
- Full-time team at Astral working on it daily
- Modern Python syntax support (3.12+, 3.13+)
- Battle-tested in production (Ruff has 45k stars, used everywhere)
- Pin to git SHA for stability:

    ```toml
    ruff_python_parser = { git = "https://github.com/astral-sh/ruff", rev = "abc123" }
    ```

- Update SHA when you want new features/fixes

### References

- [Ruff v0.4.0 Blog Post](https://astral.sh/blog/ruff-v0.4.0)
- [GitHub: astral-sh/ruff](https://github.com/astral-sh/ruff)
- [Ruff Internals Analysis](https://compileralchemy.substack.com/p/ruff-internals-of-a-rust-backed-python)

---

## 2. RustPython Parser (`rustpython-parser`)

### Overview

Originally the parser from RustPython (a Python interpreter in Rust). Now maintained as a standalone library. This was also the original parser used by Ruff before they forked and rewrote it.

### Key Findings

- **Published to crates.io** ✅ - v0.4.0 (68K SLoC)
- 3.1M+ downloads all time
- MIT licensed
- Uses LALRPOP for grammar
- Supports Python 3 syntax

### Usage Example

```rust
use rustpython_parser::{Parse, ast};

let python_source = "print('Hello world')";
let python_statements = ast::Suite::parse(python_source).unwrap();
let python_expr = ast::Expr::parse(python_source).unwrap();
```

### Real-World Usage

**pytest-language-server** uses this parser:

```
- **Parser**: rustpython-parser
- **LSP Framework**: tower-lsp-server
- **Concurrency**: tokio async runtime
```

**pylyzer** (Python type checker/LSP) also uses it:

```toml
rustpython-parser = { git = "https://github.com/RustPython/Parser", version = "0.4.0", features = ["all-nodes-with-ranges", "location"] }
```

### Maintenance Activity

- **GitHub Stars:** 113
- **Last crates.io Release:** 2024-08-06 (v0.4.0) - **18 months ago!**
- **Last GitHub Commit:** 2025-08-20 - 6 months ago
- **Open Issues:** 36
- **Release History:**
    - v0.4.0 - 2024-08-06
    - v0.3.1 - 2024-04-06
    - v0.3.0 - 2023-08-29
    - v0.2.0 - 2023-01-11

### Viability Assessment: **⚠️ Caution - Essentially Abandonware**

- README explicitly states: "superseded by https://github.com/astral-sh/ruff/tree/v0.4.10/crates/ruff_python_parser"
- No crates.io release in 18 months
- Sporadic GitHub commits (minor fixes only)
- **May not support Python 3.12+ syntax** (type parameter syntax, etc.)
- Still used by pytest-language-server and pylyzer, but both use git dependency, not crates.io

### Why Projects Still Use It

- pylyzer uses git dependency: `rustpython-parser = { git = "https://github.com/RustPython/Parser" }`
- pytest-language-server was built recently but may face issues with newer Python syntax

### References

- [crates.io: rustpython-parser](https://crates.io/crates/rustpython-parser)
- [docs.rs documentation](https://docs.rs/rustpython-parser)
- [GitHub: RustPython/Parser](https://github.com/RustPython/Parser)
- [Parser Blog Post](https://rustpython.github.io/blog/2020/04/02/thing-explainer-parser.html)

---

## 3. Tree-sitter Python (`tree-sitter-python`)

### Overview

Grammar for tree-sitter, the incremental parsing library. **Produces CST (Concrete Syntax Tree), not AST.**

### Why It's Not Suitable for AST Parsing

Tree-sitter is designed for editor features like syntax highlighting and incremental re-parsing. It produces a CST that:

- Preserves all tokens (whitespace, comments, punctuation)
- Doesn't abstract away syntactic details
- Requires manual traversal to extract semantic meaning

For precise AST parsing needs, this is the wrong tool.

### When Tree-sitter IS Appropriate

- Syntax highlighting
- Bracket matching
- Incremental parsing during typing
- Error-tolerant parsing of incomplete code

### References

- [crates.io: tree-sitter-python](https://crates.io/crates/tree-sitter-python)
- [Tree-sitter Documentation](https://tree-sitter.github.io/)

---

## 4. python-ast Crate

### Overview

Uses PyO3 to call Python's own `ast` module, then converts to Rust types.

### Key Findings

- **Published to crates.io** ✅ - v1.0.2
- 21K downloads
- Apache-2.0 licensed
- Requires Python runtime (PyO3)
- Also has experimental Python-to-Rust transpilation

### Usage Example

```rust
use python_ast::parse;

let ast = parse("def hello(): return 'world'", "hello.py")?;
println!("Parsed {} statements", ast.raw.body.len());
```

### Maintenance Activity

- **GitHub Stars:** 12
- **Last Commit:** 2025-08-06 - 6 months ago
- **Open Issues:** 1
- **Activity:** Solo developer project, sporadic updates

### Viability Assessment: **❌ Not Recommended**

- Requires Python runtime at parsing time
- PyO3 adds complexity
- Performance overhead from Python interop
- Very low adoption (12 stars, 21K downloads vs 3M+ for others)
- Essentially a solo hobby project

### References

- [crates.io: python-ast](https://crates.io/crates/python-ast)
- [GitHub: rexlunae/python-ast-rs](https://github.com/rexlunae/python-ast-rs)

---

## 5. pytest-language-server Architecture

This project is highly relevant as a reference implementation.

### Stack

- **Language**: Rust
- **LSP Framework**: tower-lsp-server
- **Parser**: rustpython-parser
- **Concurrency**: tokio async runtime
- **Data Structures**: DashMap for lock-free concurrent access

### Features Implemented

- Go to Definition
- Find References
- Hover Documentation
- Code Completion
- Diagnostics
- Document Symbols
- Workspace Symbols
- Call Hierarchy
- Inlay Hints

### Maintenance Activity

- **GitHub Stars:** 72
- **Last Commit:** 2026-02-04 (today)
- **Open Issues:** 0
- **Activity:** Very active, regular dependency updates

### Key Insight

The pytest-language-server successfully uses rustpython-parser for a production LSP. However, it's worth noting:

- The project is actively maintained (commits today)
- But it depends on rustpython-parser which hasn't been updated in 18 months
- This could become a problem for Python 3.12+ syntax support
- Worth watching if they switch parsers

### References

- [GitHub: bellini666/pytest-language-server](https://github.com/bellini666/pytest-language-server)

---

## Comparison Summary

### Maintenance Status (as of 2026-02-04)

| Parser             | Last Release   | Last Commit | Stars | Verdict        |
| ------------------ | -------------- | ----------- | ----- | -------------- |
| ruff_python_parser | N/A (internal) | Today       | 45.6k | 🟢 Very active |
| tree-sitter-python | 2025-09-11     | 2025-09-15  | 520   | 🟢 Active      |
| rustpython-parser  | 2024-08-06     | 2025-08-20  | 113   | 🔴 Stale       |
| python-ast         | 2025-08        | 2025-08-06  | 12    | 🔴 Abandoned   |

### Performance (estimated, relative)

1. ruff_python_parser (fastest, hand-written recursive descent)
2. tree-sitter-python (fast, incremental updates)
3. rustpython-parser (good, LALRPOP-based)
4. python-ast (slowest, Python interop)

### For django-language-server

**Recommendation: Use ruff_python_parser**

```toml
[dependencies]
ruff_python_parser = { git = "https://github.com/astral-sh/ruff", rev = "<pin-to-sha>" }
ruff_python_ast = { git = "https://github.com/astral-sh/ruff", rev = "<pin-to-sha>" }
```

| Option                 | Verdict                                               |
| ---------------------- | ----------------------------------------------------- |
| **ruff_python_parser** | ✅ **Use this** - only actively maintained AST parser |
| **rustpython-parser**  | ❌ Stale (18mo), may lack Python 3.12+                |
| **python-ast**         | ❌ Requires Python runtime                            |
| ~~tree-sitter-python~~ | ❌ CST, not AST - wrong tool for the job              |

### Feature Requirements

| Requirement            | ruff_python_parser               |
| ---------------------- | -------------------------------- |
| Python 3.12+ syntax    | ✅ Yes (Ruff supports it)        |
| Python 3.13+ syntax    | ✅ Likely (Astral keeps current) |
| Source locations/spans | ✅ Yes (required for Ruff)       |
| f-string parsing       | ✅ Yes                           |
| Type annotations       | ✅ Yes                           |
| Error recovery         | ❓ Need to verify                |

---

## Next Steps

1. Explore ruff_python_parser API - what does parsing look like?
2. Check source location/span info quality for LSP use
3. Look at how Ruff/ty traverse the AST - visitor pattern?
4. Find a good SHA to pin to (recent stable tag?)

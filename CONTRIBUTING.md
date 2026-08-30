# Contributing

All contributions are welcome! Besides code contributions, this includes things like documentation improvements, bug reports, and feature requests.

You should first check if there is a [GitHub issue](https://github.com/joshuadavidthomas/django-language-server/issues) already open or related to what you would like to contribute. If there is, please comment on that issue to let others know you are working on it. If there is not, please open a new issue to discuss your contribution.

Not all contributions need to start with an issue, such as typo fixes in documentation or version bumps to Python or Django that require no internal code changes, but generally, it is a good idea to open an issue first.

We adhere to a version of Django's Code of Conduct in all interactions and expect all contributors to do the same. Please read the [Code of Conduct](https://github.com/joshuadavidthomas/django-language-server?tab=coc-ov-file) before contributing.

## AI Policy

Someone is going to read your PR. Be considerate of that — make sure what you're submitting is something you'd want to review yourself.

AI tools are fine to use. How the code got written matters less than whether it's good. But you're the one submitting it, so you're the one responsible for it. If you can't explain a change, don't submit it. If you haven't tested it, don't submit it. If it doesn't fit the codebase, it's going to need rework.

Mentioning that you used AI is appreciated but not required. We'll assume good faith. That said, a pattern of sloppy submissions speaks for itself regardless of how the code was produced.

- If you submit it, you own it. "The AI wrote it" is not an explanation.
- Read the diff. Understand what it does and why.
- Test your work. Don't submit code you haven't verified.
- Make sure it fits — existing patterns, naming conventions, architecture.

The project includes an [`AGENTS.md`](AGENTS.md) file with guidelines for AI coding agents. If you're using an AI tool that supports it, point it there.

Before opening a PR, make sure the tests, clippy, formatting, and linting all pass.

## Getting oriented

Django Language Server is exactly what the name says: a standalone program that editors start in the background and query over the Language Server Protocol (LSP). If you have never worked with a language server before, start with this section; the documents linked at the end go deeper.

### The editor/server split

The editor owns presentation: completion menus, squiggly underlines, hover popups, jumping between files. The server owns analysis: parsing templates, validating tags and filters, resolving `{% extends %}` chains, knowing which template tag libraries the project can load. The two communicate only through JSON-RPC messages whose shapes the [Language Server Protocol](https://microsoft.github.io/language-server-protocol/) defines, and neither knows the other's internals. That separation lets one server support every editor. It also splits responsibility cleanly when debugging: how a result is displayed is editor behavior; what the result contains is decided in this repository.

A typical session, condensed:

1. The editor spawns `djls serve` as a child process and speaks JSON-RPC over its stdin and stdout.
2. An `initialize` exchange negotiates capabilities: completion, hover, diagnostics, go to definition, and so on.
3. The server statically reads the project (settings module, `INSTALLED_APPS`, template directories, template tag libraries). It never imports or runs project code.
4. Opening a template sends `textDocument/didOpen` with the file's full text. The server analyzes it and pushes back diagnostics, which the editor draws as squiggles.
5. Each keystroke sends `textDocument/didChange`. The server re-analyzes from the editor's buffer, not the file on disk, so it sees unsaved changes.
6. Hover, completion, and go to definition are request/response pairs: the editor sends a position, the server answers from its analysis, and the editor renders the result.

### Inside the server

The codebase is a Cargo workspace of small crates, layered so that each answers a different kind of question. Two kinds of knowledge feed everything: what tags and filters *exist* (read from the Python side of the project) and what the template *says* (parsed from the template source). Separate subsystems produce each, and they meet in the middle during semantic analysis.

From the bottom up:

| Crate | Answers |
|---|---|
| `djls-source` | Files, spans, line indexes, filesystem access. Nearly everything depends on it. |
| `djls-project` | Project facts: Python environment discovery, settings extraction, template directories, template tag libraries, and the static extraction that derives validation rules (argument counts, block structure, filter arity) from the Python source of template tag libraries. |
| `djls-templates` | Template syntax. A hand-written recursive descent parser that knows nothing about Django semantics and never fails: parse errors become error nodes in its output, because the user is always mid-keystroke in something invalid and the rest of the pipeline has to keep working. |
| `djls-semantic` | Project meaning. Parsed templates meet project facts here: which libraries are loaded at each position, whether a tag is valid where it appears, structural diagnostics. |
| `djls-ide` | Translation. Turns analysis into LSP-shaped answers: completions, diagnostics, definitions, references. Everything below it is LSP-unaware. |
| `djls-server` | The protocol. The only crate that speaks LSP: the session, open-document buffers, request handling. The JSON-RPC transport and request dispatch come from [tower-lsp-server](https://github.com/tower-lsp-community/tower-lsp-server); this crate implements the handlers on top. |
| `djls` | The CLI. `djls serve` starts the server; `djls check` runs the same validation in a terminal. |

Tying the layers together is [Salsa](https://github.com/salsa-rs/salsa), the incremental computation framework also used by rust-analyzer. You write analysis as queries over inputs, and when a file changes only the affected queries recompute. That keeps re-analysis on every keystroke cheap.

A template flows through the pipeline in stages: lexing, parsing into a flat node list, analysis (building the template tree and working out which libraries each position can see), validation, diagnostics. No stage blocks on errors from a previous one. A template full of syntax errors still gets structural analysis on its valid portions, and a template with structural problems still gets validation on the tags that parsed correctly.

[ARCHITECTURE.md](ARCHITECTURE.md) has the full map (per-crate detail, the database design, and the invariants the layering maintains), and [CONTEXT.md](CONTEXT.md) is the domain glossary: the canonical name for every concept in the codebase.

## New to Rust?

The server is written in Rust, but this is a project *for* Django developers, and Django expertise is just as valuable as Rust expertise. Understanding Django's internals and common development patterns helps shape what features would be most valuable and how they should behave.

If you know Python but not Rust and want to contribute code:

- [The Rust Book](https://doc.rust-lang.org/book/) is the standard introduction, free and worth reading in order.
- [Rustlings](https://github.com/rust-lang/rustlings) is a set of small exercises that pairs well with the book.
- The one unusual dependency here is [Salsa](https://salsa-rs.github.io/salsa/), the incremental computation framework also used by rust-analyzer and ty. Most changes don't require understanding it; read its book when you start touching query code.

Plenty of valuable contributions require no Rust at all: editor client configurations, documentation, bug reports with a reproducing template, and feedback on how features behave in real Django projects.

So far it's all been built by [a simple country CRUD web developer](https://youtu.be/7ij_1SQqbVo?si=hwwPyBjmaOGnvPPI&t=53) learning Rust along the way — send help!

## Development

The project uses a [Cargo workspace](https://doc.rust-lang.org/cargo/reference/workspaces.html) with all crates under `crates/`. A few conventions to be aware of:

- **Dependency versions** are centralized in `[workspace.dependencies]` in the root [`Cargo.toml`](./Cargo.toml). Individual crates reference them with `dep.workspace = true` and never specify versions directly.
- **Internal crates are listed before third-party crates** in each crate's `[dependencies]`, separated by a blank line. Both groups are kept in alphabetical order.
- **Lints** are configured once in `[workspace.lints]` in the root `Cargo.toml`. Each crate opts in with `[lints] workspace = true`.
- **Versioning**: Only the `djls` binary crate carries the release version. All library crates use `version = "0.0.0"`.

### First-time setup

Development requires [Rustup](https://rustup.rs/), [uv](https://docs.astral.sh/uv/), and [just](https://just.systems/). The checked-in Rust toolchain files select the required compiler and formatter versions.

Install the locked Python development dependencies without building the local Rust package, install the Git hooks, and prefetch the test corpus:

```bash
uv sync --frozen --no-install-project
uv tool install prek
prek install
just corpus sync
```

Install the prebuilt snapshot review tool used throughout the test suite:

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/mitsuhiko/insta/releases/download/1.48.0/cargo-insta-installer.sh | sh
```

The first test or lint run may still download a supported Python version, create Nox environments, compile the Rust workspace, and prepare hook environments. Subsequent runs reuse those artifacts. Amp orbs perform these setup steps automatically through `.agents/setup`.

### The core loop

Three commands cover almost all day-to-day work:

| Command | When |
|---|---|
| `cargo test -q` | After every change; runs the Rust workspace tests against your current Python environment |
| `cargo insta review` | After tests report snapshot changes; review them interactively |
| `just lint` | Before committing; formats and runs every lint hook, including Rustfmt and Clippy |

Everything else documented below exists for specific situations: cross-version testing, LSP end-to-end coverage, visibility audits, profiling. Reach for those when the situation comes up, not routinely.

### Testing

| Command | Scope |
|---|---|
| `cargo test -q` | Rust workspace tests using the currently discoverable Python environment |
| `just test` | Rust workspace tests with the default Python 3.10 and Django 5.2 environment |
| `just testall` | All supported Python and Django combinations |
| `just e2e` | Python LSP end-to-end tests |

`just test` and `just testall` create isolated Nox environments, install the selected Django version, synchronize the corpus, and then run Cargo. Use `just testall` for Python/Django support changes; the default `just test` is the normal local compatibility check. Use `just e2e` when changing behavior an editor observes: initialization, diagnostics, navigation, completions.

#### Corpus

The corpus contains pinned source from real Django packages and projects under `crates/djls-testing/.corpus`. Tests synchronize it automatically, while `just corpus sync` can prefetch or repair it explicitly. The first sync downloads dozens of checksum-validated archives and can consume hundreds of megabytes; later syncs skip entries that already match `crates/djls-testing/manifest.lock`.

#### Snapshots

The test suite uses [Insta](https://insta.rs/) snapshots extensively. After running the relevant tests, inspect pending changes interactively:

```bash
cargo insta review
```

To rerun snapshot tests, accept updates, and delete unreferenced snapshots in one noninteractive pass:

```bash
cargo insta test --accept --unreferenced delete
```

Always review snapshot changes before committing them.

### Linting

Install the commit-time hooks with `prek install`. Run `just lint` for the all-files local gate; it formats the Justfiles and runs every configured hook, including Rustfmt and Clippy. CI runs the portable pre-commit hooks, Rustfmt, Clippy, and Hawk as separate jobs.

#### Formatting

Formatting uses the dated nightly pinned in [`tools/rustfmt/rust-toolchain.toml`](tools/rustfmt/rust-toolchain.toml) because the repository enables unstable rustfmt options. Run `just fmt` so local formatting uses that toolchain. Update the pin deliberately when newer Rust syntax or rustfmt fixes require it, then review and commit any resulting formatting changes.

#### Visibility Audits

[Hawk](https://github.com/astral-sh/hawk) is an experimental Cargo lint from Astral that checks unnecessary public Rust visibility across a closed-world workspace. It is useful here because most crates are internal architecture layers behind the shipped `djls` binary.

It matters most when you are changing public APIs, moving code across crates, or cleaning up visibility. A change that stays completely inside one crate is less important. Each run performs multiple Cargo passes and is heavy on CPU and disk. If you are new to the project, let CI run it: a `hawk` job checks every pull request.

##### Setup

Install the Cargo subcommand Hawk expects. Rustup installs the compiler pinned for Hawk when the recipe runs.

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/astral-sh/hawk/releases/download/0.1.9/cargo-hawk-installer.sh | sh
```

##### Usage

Run Hawk through `just` rather than `cargo hawk` directly:

```bash
just hawk
```

The recipe uses the exact compiler pinned in [`tools/hawk/rust-toolchain.toml`](tools/hawk/rust-toolchain.toml), as required by cargo-hawk 0.1.9, and isolates Hawk's instrumented builds to avoid [astral-sh/hawk#74](https://github.com/astral-sh/hawk/issues/74).

The multiple passes come from Hawk checking the configured production binaries and workspace non-production targets. `--fix` can repeat analysis while visibility changes converge. That cost is expected: Hawk answers a different question than clippy, namely whether crate boundaries expose more API surface than the workspace needs.

The `just hawk` recipe keeps rustc dead-code and unused-import warnings quiet so the output stays focused on visibility. After applying Hawk fixes, run the normal lint and test checks; newly private code may expose cleanup work that belongs there.

### Debug information

Development and test builds use line-table-only debug information to keep Rust build artifacts smaller while retaining file-and-line panic backtraces and source-level stepping. Compiler diagnostics and normal build and test behavior are unaffected, but native debuggers cannot inspect local variables and function arguments.

When full GDB or LLDB inspection is needed, override the relevant Cargo profile for that build:

```bash
CARGO_PROFILE_DEV_DEBUG=full cargo build
CARGO_PROFILE_TEST_DEBUG=full cargo test
```

### Profiling

You will rarely need this; it is for benchmark investigations, not everyday changes.

#### Setup

You'll need `jq`, `rg`, and the **codspeed fork of valgrind** (not stock valgrind):

```bash
git clone --depth 1 https://github.com/CodSpeedHQ/valgrind-codspeed /tmp/valgrind-codspeed
cd /tmp/valgrind-codspeed
./autogen.sh
./configure --prefix=$HOME/.local
make -j$(nproc)
make install
```

Make sure `$HOME/.local/bin` is on your `PATH`. Verify with:

```bash
valgrind --version  # should contain "codspeed"
```

#### Usage

The `just dev profile` command runs benchmarks under [valgrind-codspeed](https://github.com/CodSpeedHQ/valgrind-codspeed), the same callgrind fork used in CI. It produces deterministic per-function instruction counts with call trees, and automatically strips harness overhead.

```bash
just dev profile <bench> [filter]

# Examples:
just dev profile diagnostics collect_diagnostics_realistic
just dev profile parser parse_template
```

## Changelog

The project maintains a [`CHANGELOG.md`](CHANGELOG.md) following [Keep a Changelog](https://keepachangelog.com/en/1.0.0/). All notable changes should be documented under the `[Unreleased]` heading in the appropriate section.

**Sections** (use only those that apply):

- `Added` — new features
- `Changed` — changes in existing functionality
- `Deprecated` — soon-to-be removed features
- `Removed` — now removed features
- `Fixed` — bug fixes
- `Security` — vulnerability fixes

**Writing entries:**

- Keep entries short and factual — describe what changed, not why
- Use past tense verbs: "Added", "Fixed", "Removed", "Bumped", etc.
- Wrap crate names, types, commands, and config keys in backticks
- Prefix internal changes (refactors, crate restructuring, CI) with `**Internal**:`
- List user-facing entries before `**Internal**:` entries within each section

**Examples:**

```markdown
### Added

- Added `diagnostics.severity` configuration option for configuring diagnostic severity levels.

### Changed

- Bumped Rust toolchain from 1.90 to 1.91.
- **Internal**: Extracted concrete Salsa database into new `djls-db` crate.

### Fixed

- Fixed false positive errors for quoted strings with spaces (e.g., `{% translate "Contact the owner" %}`).
```

## Maintainer reference

Version-support updates (adding or dropping Python and Django versions) and development-tool pin updates are documented in [MAINTAINING.md](MAINTAINING.md).

## `Justfile`

The repository includes a [`Justfile`](./Justfile) that provides all common development tasks with a consistent interface. Running `just` without arguments shows all available commands and their descriptions.

<!-- [[[cog
import subprocess
import cog

output_raw = subprocess.run(["just", "--list", "--list-submodules"], stdout=subprocess.PIPE)
output_list = output_raw.stdout.decode("utf-8").split("\n")

cog.outl("""\
```bash
$ just
$ # just --list --list-submodules
""")

for i, line in enumerate(output_list):
    if not line:
        continue
    cog.out(line)
    if i < len(output_list):
        cog.out("\n")

cog.out("```")
]]] -->
```bash
$ just
$ # just --list --list-submodules

Available recipes:
    bumpver *ARGS
    check *ARGS
    clean
    clippy *ARGS
    corpus *ARGS
    e2e *ARGS
    fixtures *ARGS
    fmt *ARGS
    hawk *ARGS
    lint *ARGS     # run pre-commit on all files
    run *ARGS
    test *ARGS
    testall *ARGS
    dev:
        debug                      # TODO: djls-tmux binary was removed in #214, this recipe needs updating
        explore FILENAME="djls.db"
        inspect
        profile bench filter=""    # Profile a bench with callgrind
        record FILENAME="djls.db"
    docs:
        build LOCATION="site" # Build documentation
        serve PORT="8000"     # Serve documentation locally
```
<!-- [[[end]]] -->

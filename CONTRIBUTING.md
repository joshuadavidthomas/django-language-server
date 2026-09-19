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
5. Document edits send `textDocument/didChange` notifications, which the editor may batch. The server re-analyzes from the editor's buffer, not the file on disk, so it sees unsaved changes.
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

Fork the repository on GitHub and clone your fork. Run the setup commands below from the checkout's root.

Install [mise](https://mise.jdx.dev/getting-started.html) and [activate it in your shell](https://mise.jdx.dev/getting-started.html#activate-mise). From the repository root, install the development tools:

```bash
mise trust
mise install
mise -C tools/rustfmt install rust
```

This installs Rust, Just, uv, prek, cargo-insta, Hawk, and zizmor. For noninteractive shells and editors, [add mise's shims to `PATH`](https://mise.jdx.dev/dev-tools/shims.html); alternatively prefix commands with `mise exec --`.

Tool versions are defined in [`mise.toml`](mise.toml) and the checked-in `rust-toolchain.toml` files.

Install the locked Python development dependencies without building the local Rust package, fetch the Rust dependencies, install the Git hooks, and prefetch the test corpus:

```bash
uv sync --frozen --no-install-project
cargo fetch --locked
prek install
just corpus sync
```

The first test or lint run may still download a supported Python version, create Nox environments, compile the Rust workspace, and prepare hook environments. Subsequent runs reuse those artifacts.

### Make your first change

Create a branch and run `just test` before editing to establish a working baseline. This is the normal test command: it prepares the default Python/Django environment and corpus, then runs the Rust suite. Subsequent runs reuse the environment, so you do not need to manage it yourself.

Use the crate map above to find the code that owns the behavior. The [test-layer overview](ARCHITECTURE.md#testing) explains where parser, semantic, corpus, and LSP tests live; start with a nearby test and add a case that reproduces the bug or exercises the new behavior.

During development, run the relevant crate or test rather than the whole workspace each time:

```bash
just test -p djls-templates
# Replace the crate and test name with the ones you are working on:
just test -p <crate> <test_name>
```

For changes to analysis or editor behavior, [try the development server](#try-your-development-server) as well. Automated tests cover regressions; running your build lets you check that it behaves as intended in a Django project.

The routine checks are:

| Command | When |
|---|---|
| `just test` | During development and before opening a PR; run the full Rust suite after focused tests pass |
| `just fmt` | After editing Rust; applies formatting using the pinned formatter |
| `just lint` | Before committing; runs the all-files lint hooks, including Rust formatting checks and Clippy. It does not fix Rust formatting |

When a change touches snapshots, review them with `cargo insta review` before committing; see [Snapshots](#snapshots).

Before opening your PR:

- Run the checks above and any additional tests relevant to the change, using the [test-command table](#testing).
- Review the diff, including accepted snapshots, and remove accidental changes.
- Add a [changelog entry](#changelog) for notable changes.
- Explain the behavior changed, link the related issue, and state how you tested it.

### Try your development server

Use `cargo run` to build and run your changes in one command. In the examples below, replace `/path/to/django-language-server` with the absolute path to your checkout. `--manifest-path` selects that checkout while leaving the working directory in the Django project you want to analyze, so you do not accidentally test an installed release.

Use a Django project with its dependencies installed for these checks, rather than opening this Rust repository as the project to analyze. Configure its Django settings module through `DJANGO_SETTINGS_MODULE` or a project configuration file. For example, in that Django project's `pyproject.toml`:

```toml
[tool.djls]
django_settings_module = "myproject.settings"
```

Replace `myproject.settings` with the project's module. The server searches standard virtual environment directories such as `.venv`; set `venv_path` if the environment lives elsewhere. See [Configuration](docs/configuration/index.md) for details. The Django project's environment is separate from this repository's development environment.

#### In a terminal

From the Django project root, check a template:

```bash
cargo run --manifest-path /path/to/django-language-server/Cargo.toml -p djls -- check templates/example.html
```

Replace the template path with a file in your project. For a quick smoke check, add `{% block content %}` without its closing tag: the command should report an unclosed tag and exit with status 1. Add `{% endblock %}` and check that the diagnostic disappears. Then try the behavior your change affects. See the [CLI guide](docs/cli.md) for other inputs and options.

#### In an editor

Configure your editor's LSP client to use `cargo run` with `serve` as the server argument. Override any installed or automatically downloaded server so only your development build is attached. Cargo must be available on the editor's `PATH`. For example, with Neovim 0.11+, put this in `init.lua`:

```lua
vim.lsp.config('djls', {
  cmd = {
    'cargo', 'run', '--quiet',
    '--manifest-path', '/path/to/django-language-server/Cargo.toml',
    '-p', 'djls', '--', 'serve',
  },
  filetypes = { 'htmldjango', 'html', 'python' },
  root_markers = { 'manage.py', 'pyproject.toml', '.git' },
})
vim.lsp.enable('djls')
```

Open the Django project and a template within it. In Neovim, `:checkhealth vim.lsp` helps confirm that the client is attached; check `:set filetype?` if it is not. The [editor guides](docs/clients/index.md) cover client setup, including [Neovim filetype detection](docs/clients/neovim.md#file-type-detection). Other editors have their own executable-override settings.

Try the unclosed-block example above and confirm that its diagnostic appears and disappears as you edit, without saving. Then reproduce the behavior you are changing.

**After every Rust change, restart the editor's language-server process** (or restart the editor). `cargo run` rebuilds changed code on startup; the already-running server does not reload your edits. The first launch may take longer while Cargo compiles dependencies.

From the server repository root, `just run` is shorthand for `cargo run -p djls --`. Running `just run serve` in a terminal waits for LSP messages on stdin/stdout: it is not an interactive shell or a web server. Normally, let the editor start `serve` for you.

### Testing

Use `just test` by default, including for focused runs. It forwards crate and test-name filters to Cargo. The other test commands are for additional coverage:

| Command | What it runs | When to choose it |
|---|---|---|
| `just test` | The Rust suite excluding corpus-wide sweeps, in a Nox-managed Python 3.10/Django 5.2 environment, after synchronizing corpus source | Everyday development and the pre-PR check |
| `just testall` | The Rust suite in each configured compatible Python/Django combination, including Django `main` | Changing version support or investigating a matrix-specific failure; this does not include LSP end-to-end tests |
| `uv run --no-project --with 'nox[uv]' nox -s corpus` | Prepare source and dependencies, then run all six corpus-wide suites | Checking real projects in their own dependency environments, once outside the version matrix |
| `just e2e` | Python/pytest LSP end-to-end tests against the checkout, in the default Python/Django environment | Changing editor-visible behavior such as initialization, diagnostics, navigation, or completions |

`just test` and `just testall` create or reuse isolated Nox environments and install the selected Django version before running Cargo. This does not mean every Rust test analyzes that installed Django version: source-backed fixtures use pinned corpus files or explicit test data. The version matrix and incompatible combinations are defined in [`noxfile.py`](noxfile.py).

You may see `cargo test` in Rust documentation. It runs the Rust suite directly, without the environment setup or corpus synchronization provided by `just test`. You do not need to use it separately in the normal contribution workflow.

#### Corpus

The complete suite requires the corpus: pinned project source and dependencies under `crates/djls-testing/.corpus`. Run `just corpus sync` for setup. Subsequent syncs reuse unchanged checkouts and ready environments, rebuilding only missing or stale entries. Use `just corpus sync --refresh` to deliberately refresh dependencies, including unlocked upstream requirements. No additional dependency lockfiles are maintained by the corpus.

`just test` and `just testall` sync source for their focused tests but leave the six corpus-wide sweeps to the separate Nox `corpus` session. Those sweeps use each project's own environment where import resolution matters, so repeating them under every ambient Python/Django version adds no coverage. Plain `cargo test` includes them and therefore needs complete setup. Isolated tests such as `cargo test -p djls-templates` can run without the corpus; that subset is not a substitute for real-project extraction coverage.

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

Formatting uses the dated nightly pinned in [`tools/rustfmt/rust-toolchain.toml`](tools/rustfmt/rust-toolchain.toml) because the repository enables unstable rustfmt options. Run `just fmt` to apply formatting using that toolchain; `just lint` only checks Rust formatting. Changing toolchain pins is covered under [Maintaining](#updating-development-tools).

#### Visibility Audits

[Hawk](https://github.com/astral-sh/hawk) is an experimental Cargo lint from Astral that checks unnecessary public Rust visibility across a closed-world workspace. It is useful here because most crates are internal architecture layers behind the shipped `djls` binary.

It matters most when you are changing public APIs, moving code across crates, or cleaning up visibility. A change that stays completely inside one crate is less important. Each run performs multiple Cargo passes and is heavy on CPU and disk. If you are new to the project, let CI run it: a `hawk` job checks pull requests affecting Rust code or its tooling.

##### Usage

Run Hawk through `just` rather than `cargo hawk` directly:

```bash
just hawk
```

The recipe uses the compiler pinned in [`tools/hawk/rust-toolchain.toml`](tools/hawk/rust-toolchain.toml), as required by the cargo-hawk version in `mise.toml`, and isolates Hawk's instrumented builds to avoid [astral-sh/hawk#74](https://github.com/astral-sh/hawk/issues/74). It enforces findings with `-D warnings`, the same contract as the other lint recipes. Rustup installs that compiler on first use if needed.

The multiple passes come from Hawk checking the configured production binaries and workspace non-production targets. `--fix` can repeat analysis while visibility changes converge. That cost is expected: Hawk answers a different question than clippy, namely whether crate boundaries expose more API surface than the workspace needs.

Hawk is installed by `mise install` during setup; `just hawk` runs it without updating it. Coordinated tool updates belong under [Maintaining](#updating-development-tools). After applying Hawk fixes, run the normal lint and test checks; newly private code may expose cleanup work that belongs there.

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

The `just dev profile` command runs benchmarks under [valgrind-codspeed](https://github.com/CodSpeedHQ/valgrind-codspeed), the same callgrind fork used in CI. It records per-function instruction counts with call trees, and automatically strips harness overhead.

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

## Maintaining

These procedures cover maintenance work outside the normal contribution flow: adding or dropping supported Python and Django versions, and updating pinned development tools.

### Version updates

#### Python

The project uses [`noxfile.py`](noxfile.py) as the single source of truth for supported Python versions. The `PY_VERSIONS` list in this file controls:

- **Auto-generated documentation**: [cogapp](https://nedbatchelder.com/code/cog/) reads `PY_VERSIONS` to generate Python version classifiers in [`pyproject.toml`](pyproject.toml) and the supported versions list in [`README.md`](README.md)
- **CI/CD test matrix**: GitHub Actions workflows call the `gha_matrix` nox session to generate the test matrix from `PY_VERSIONS`, so all supported Python versions are tested automatically
- **Local testing**: The `tests` nox session uses `PY_VERSIONS` to parametrize test runs across all supported Python versions

> [!NOTE]
> When possible, prefer submitting additions and removals in separate pull requests. This makes it easier to review changes and track the impact of each version update independently.

**To update the list of supported Python versions:**

1. Update [`noxfile.py`](noxfile.py), adding or removing version constants as needed and updating the `PY_VERSIONS` list accordingly.

    For example, to add Python 3.14 and remove Python 3.9:

    ```diff
    -PY39 = "3.9"
     PY310 = "3.10"
     PY311 = "3.11"
     PY312 = "3.12"
     PY313 = "3.13"
    -PY_VERSIONS = [PY39, PY310, PY311, PY312, PY313]
    +PY314 = "3.14"
    +PY_VERSIONS = [PY310, PY311, PY312, PY313, PY314]
    ```

2. Regenerate auto-generated content:

    ```bash
    just cog
    ```

    This updates:

    - The `requires-python` field in [`pyproject.toml`](pyproject.toml)
    - Python version trove classifiers in [`pyproject.toml`](pyproject.toml)
    - Supported versions list in [`README.md`](README.md)

3. Update the lock file:

    ```bash
    uv lock
    ```

4. Test the changes:

    ```bash
    just testall
    ```

    Use `just testall` rather than `just test` to ensure all Python versions are tested. The `just test` command only runs against the default versions (the oldest supported Python and Django LTS) and won't catch issues with newly added versions.

    Alternatively, you can test only a specific Python version across all Django versions by `nox` directly:

    ```bash
    nox --python 3.14 --session tests
    ```

5. Update [`CHANGELOG.md`](CHANGELOG.md), adding entries for any versions added or removed.

#### Django

The project uses [`noxfile.py`](noxfile.py) as the single source of truth for supported Django versions. The `DJ_VERSIONS` list in this file controls:

- **Auto-generated documentation**: [cogapp](https://nedbatchelder.com/code/cog/) reads `DJ_VERSIONS` to generate Django version classifiers in [`pyproject.toml`](pyproject.toml) and the supported versions list in [`README.md`](README.md)
- **CI/CD test matrix**: GitHub Actions workflows call the `gha_matrix` nox session to generate the test matrix from `DJ_VERSIONS`, so all supported Django versions are tested automatically
- **Local testing**: The `tests` nox session uses `DJ_VERSIONS` to parametrize test runs across all supported Django versions

> [!NOTE]
> When possible, prefer submitting additions and removals in separate pull requests. This makes it easier to review changes and track the impact of each version update independently.

**To update the list of supported Django versions:**

1. Update [`noxfile.py`](noxfile.py), adding or removing version constants as needed and updating the `DJ_VERSIONS` list accordingly.

    For example, to add Django 6.1 and remove Django 4.2:

    ```diff
    -DJ42 = "4.2"
     DJ51 = "5.1"
     DJ52 = "5.2"
     DJ60 = "6.0"
    +DJ61 = "6.1"
     DJMAIN = "main"
    -DJ_VERSIONS = [DJ42, DJ51, DJ52, DJ60, DJMAIN]
    +DJ_VERSIONS = [DJ51, DJ52, DJ60, DJ61, DJMAIN]
    ```

2. Update any Python version constraints in the `should_skip()` function if the new Django version has specific Python requirements.

3. Regenerate auto-generated content:

    ```bash
    just cog
    ```

    This updates:

    - Django version trove classifiers in [`pyproject.toml`](pyproject.toml)
    - Supported versions list in [`README.md`](README.md)
    - Supported versions list in [`docs/installation.md`](docs/installation.md)

4. Update the lock file:

    ```bash
    uv lock
    ```

5. Test the changes:

    ```bash
    just testall
    ```

    Use `just testall` rather than `just test` to ensure all Django versions are tested. The `just test` command only runs against the default versions (the oldest supported Python and Django LTS) and won't catch issues with newly added versions.

    Alternatively, you can test only a specific Django version across all Python versions by using `nox` directly:

    ```bash
    nox --session "tests(django='6.1')"
    ```

6. Update [`CHANGELOG.md`](CHANGELOG.md), adding entries for any versions added or removed.

7. **For major Django releases**: If adding support for a new major Django version (e.g., Django 6.0), the language server version should be bumped to match per [DjangoVer](docs/versioning.md) versioning. For example, when adding Django 6.0 support, bump the server from v5.x.x to v6.0.0.

### Updating development tools

- Update the primary compiler in `rust-toolchain.toml`.
- Update the formatter nightly in `tools/rustfmt/rust-toolchain.toml`, then run `just fmt` and review any formatting changes.
- Update Hawk in `mise.toml` together with its required compiler in `tools/hawk/rust-toolchain.toml`, then run `mise install` and `just hawk`. CI uses the same pins.
- Update auxiliary tool versions in `mise.toml`, then run `mise install`. Keep cargo-insta aligned with the Insta version resolved in `Cargo.lock`.

Hawk uses compiler-private APIs, so even a patch-level compiler mismatch can make it fail before analysis. When updating it, use the compiler version named in the [Hawk release notes](https://github.com/astral-sh/hawk/releases).

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

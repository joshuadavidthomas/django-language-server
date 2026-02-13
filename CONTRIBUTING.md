# Contributing

All contributions are welcome! Besides code contributions, this includes things like documentation improvements, bug reports, and feature requests.

You should first check if there is a [GitHub issue](https://github.com/joshuadavidthomas/django-language-server/issues) already open or related to what you would like to contribute. If there is, please comment on that issue to let others know you are working on it. If there is not, please open a new issue to discuss your contribution.

Not all contributions need to start with an issue, such as typo fixes in documentation or version bumps to Python or Django that require no internal code changes, but generally, it is a good idea to open an issue first.

We adhere to Django's Code of Conduct in all interactions and expect all contributors to do the same. Please read the [Code of Conduct](https://www.djangoproject.com/conduct/) before contributing.

## Development

For a detailed look at how the codebase works — data flow, the Salsa database, the template pipeline — see [ARCHITECTURE.md](ARCHITECTURE.md).

The project is written in Rust with a Python subprocess for Django introspection. It uses a [Cargo workspace](https://doc.rust-lang.org/cargo/reference/workspaces.html) with all crates under `crates/`. A few conventions to be aware of:

- **Dependency versions** are centralized in `[workspace.dependencies]` in the root [`Cargo.toml`](./Cargo.toml). Individual crates reference them with `dep.workspace = true` and never specify versions directly.
- **Internal crates are listed before third-party crates** in each crate's `[dependencies]`, separated by a blank line. Both groups are kept in alphabetical order.
- **Lints** are configured once in `[workspace.lints]` in the root `Cargo.toml`. Each crate opts in with `[lints] workspace = true`.
- **Versioning**: Only the `djls` binary crate carries the release version. All library crates use `version = "0.0.0"`.

Code contributions are welcome from developers of all backgrounds. Rust expertise is valuable for the LSP server and core components, but Python and Django developers should not be deterred by the Rust codebase — Django expertise is just as valuable. Understanding Django's internals and common development patterns helps inform what features would be most valuable.

So far it's all been built by [a simple country CRUD web developer](https://youtu.be/7ij_1SQqbVo?si=hwwPyBjmaOGnvPPI&t=53) learning Rust along the way — send help!

## Code Quality

Someone is going to read your PR. Be considerate of that — make sure what you're submitting is something you'd want to review yourself.

AI tools are fine to use. How the code got written matters less than whether it's good. But you're the one submitting it, so you're the one responsible for it. If you can't explain a change, don't submit it. If you haven't tested it, don't submit it. If it doesn't fit the codebase, it's going to need rework.

Mentioning that you used AI is appreciated but not required. We'll assume good faith. That said, a pattern of sloppy submissions speaks for itself regardless of how the code was produced.

- If you submit it, you own it. "The AI wrote it" is not an explanation.
- Read the diff. Understand what it does and why.
- Test your work. Don't submit code you haven't verified.
- Make sure it fits — existing patterns, naming conventions, architecture.

The project includes an [`AGENTS.md`](AGENTS.md) file with guidelines for AI coding agents. If you're using an AI tool that supports it, point it there.

Before opening a PR, make sure the tests, clippy, formatting, and linting all pass.

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

## Version Updates

### Python

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

### Django

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
    fmt *ARGS
    lint          # run pre-commit on all files
    test *ARGS
    testall *ARGS
    dev:
        debug
        explore FILENAME="djls.db"
        inspect
        profile bench filter=""    # Example: just dev profile parser parse_template
        record FILENAME="djls.db"
    docs:
        build LOCATION="site" # Build documentation
        serve PORT="8000"     # Serve documentation locally
```
<!-- [[[end]]] -->

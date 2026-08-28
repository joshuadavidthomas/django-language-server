# Maintaining

Procedures for maintainers. Day-to-day development is covered in [CONTRIBUTING.md](CONTRIBUTING.md).

## Version updates

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

## Updating development tools

- Update the primary compiler in `rust-toolchain.toml`.
- Update the formatter nightly in `tools/rustfmt/rust-toolchain.toml`, then run `just fmt` and review any formatting changes.
- Update cargo-hawk in `.agents/setup` and the [CONTRIBUTING.md](CONTRIBUTING.md) install instructions together with its exact required compiler in `tools/hawk/rust-toolchain.toml`.
- Keep the prebuilt cargo-insta version in `.agents/setup` and [CONTRIBUTING.md](CONTRIBUTING.md) aligned with the Insta version resolved in `Cargo.lock`.

Hawk uses compiler-private APIs, so even a patch-level compiler mismatch can make it fail before analysis.

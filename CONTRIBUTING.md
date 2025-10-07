# Contributing

All contributions are welcome! Besides code contributions, this includes things like documentation improvements, bug reports, and feature requests.

You should first check if there is a [GitHub issue](https://github.com/joshuadavidthomas/django-language-server/issues) already open or related to what you would like to contribute. If there is, please comment on that issue to let others know you are working on it. If there is not, please open a new issue to discuss your contribution.

Not all contributions need to start with an issue, such as typo fixes in documentation or version bumps to Python or Django that require no internal code changes, but generally, it is a good idea to open an issue first.

We adhere to Django's Code of Conduct in all interactions and expect all contributors to do the same. Please read the [Code of Conduct](https://www.djangoproject.com/conduct/) before contributing.

## Development

### Version Updates

#### Python

The project uses [`noxfile.py`](noxfile.py) as the single source of truth for supported Python versions. The `PY_VERSIONS` list in this file controls:

- Auto-generated documentation: [cogapp](https://nedbatchelder.com/code/cog/) reads `PY_VERSIONS` to generate Python version classifiers in [`pyproject.toml`](pyproject.toml) and the supported versions list in [`README.md`](README.md)
- CI/CD test matrix: GitHub Actions workflows call the `gha_matrix` nox session to dynamically generate the test matrix from `PY_VERSIONS`, ensuring all supported Python versions are tested automatically
- Local testing: The `tests` nox session uses `PY_VERSIONS` to parametrize test runs across all supported Python versions

> [!NOTE]
> When possible, prefer submitting additions and removals in separate pull requests. This makes it easier to review changes and track the impact of each version update independently.

**To update the list of supported Python versions:**

1. Update [`noxfile.py`](noxfile.py), adding or removing version constants as needed and updating the `PY_VERSIONS` list accordingly.

   For example, given the following versions:

   ```python
   PY39 = "3.9"
   PY310 = "3.10"
   PY311 = "3.11"
   PY312 = "3.12"
   PY313 = "3.13"
   PY_VERSIONS = [PY39, PY310, PY311, PY312, PY313]
   ```

   To add Python 3.14 and remove Python 3.9, the final list will be:

   ```python
   PY310 = "3.10"
   PY311 = "3.11"
   PY312 = "3.12"
   PY313 = "3.13"
   PY314 = "3.14"
   PY_VERSIONS = [PY310, PY311, PY312, PY313, PY314]
   ```

2. Regenerate auto-generated content:

   ```bash
   just cog
   ```

   This updates Python version classifiers in [`pyproject.toml`](pyproject.toml) and supported versions in [`README.md`](README.md).

3. Update the ruff target-version in [`pyproject.toml`](pyproject.toml) if removing the lowest supported Python version.

   For instance, if removing Python 3.9, update:

   ```toml
   [tool.ruff]
   # Assume Python 3.10
   target-version = "py310"
   ```

   The `target-version` should match the new minimum supported Python version. This ensures `ruff`'s linting and formatting rules are appropriate for the oldest Python version supported.

4. Update the lock file:

   ```bash
   uv lock
   ```

5. Test the changes:

   ```bash
   just testall
   ```

   Use `just testall` rather than `just test` to ensure all Python versions are tested. The `just test` command only runs against the default versions (the oldest supported Python and Django LTS) and won't catch issues with newly added versions.

   If you want, you can also test only a specific Python version across all Django versions by `nox` directly:

   ```bash
   nox --python 3.14 --session tests
   ```

6. Optional: Update [`.readthedocs.yaml`](.readthedocs.yaml) if changing the documentation build Python version:

   ```yaml
   build:
     os: ubuntu-24.04
     tools:
       python: "3.14"  # Update to desired version
   ```

   > [!NOTE]
   > Before updating, verify that the new Python version is available on Read the Docs. Check the [Read the Docs documentation](https://docs.readthedocs.io/en/stable/config-file/v2.html#build-tools-python) for supported versions. If the version isn't available yet, mark this as a follow-up task to circle back to later.

7. Update [`CHANGELOG.md`](CHANGELOG.md), adding entries for any versions added or removed.

### `Justfile`

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
    fmt *ARGS
    lint          # run pre-commit on all files
    test *ARGS
    testall *ARGS
    dev:
        debug
        explore FILENAME="djls.db"
        inspect
        record FILENAME="djls.db"
    docs:
        build LOCATION="site" # Build documentation
        serve PORT="8000"     # Serve documentation locally
```
<!-- [[[end]]] -->

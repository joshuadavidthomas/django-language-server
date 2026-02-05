# Third-Party Corpus

This folder contains the manifest for assembling a local corpus of real-world
Django `templatetags` modules (and optionally templates) for regression testing.

We intentionally do **not** commit third-party source into this repository.
Downloaded corpora live under `template_linter/.corpus/` (gitignored).

## Why

Unit tests for Django itself are necessary but not sufficient:
third-party tag libraries contain additional parsing/validation patterns that
we want to support without hardcoding tag names.

## How It Works

1. Edit `template_linter/corpus/manifest.toml` to include packages and pinned versions.
2. Run `just -f template_linter/Justfile corpus-sync` to download and extract sdists.
3. Run `just -f template_linter/Justfile corpus-test` (or `just test`) to execute the corpus tests.

### Versioning Notes

For package entries, `version` can be either:
- A pinned patch version like `6.0.2`
- A minor version like `6.0` (interpreted as "latest patch for 6.0.*" at sync time)

When using a minor version, the resolved patch version is recorded in the
corpus entry's `.complete.json` as `resolved_version`.

## Licensing

Each extracted package is stored with any top-level license file(s) found in
the sdist (`LICENSE*`, `COPYING*`, `NOTICE*`), plus its build metadata
(`pyproject.toml`, `setup.cfg`, `setup.py`) when present.

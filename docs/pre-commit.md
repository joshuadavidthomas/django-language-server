# pre-commit

django-language-server provides a [pre-commit](https://pre-commit.com/) hook for running `djls check` on Django template files. This also works with [prek](https://github.com/j178/prek), a drop-in replacement for pre-commit written in Rust.

## Prerequisites

The hook uses `language: system`, which means `djls` must be installed and available on your `PATH` before running the hook. See [Installation](installation.md) for installation options.

## Usage

Add the following to your `.pre-commit-config.yaml`:

```yaml
repos:
  - repo: https://github.com/joshuadavidthomas/django-language-server
    rev: v6.0.2  # use the latest release tag
    hooks:
      - id: djls-check
```

Currently, `djls check` recognizes `.html`, `.htm`, and `.djhtml` files as templates. Files with other extensions are silently skipped.

!!! note

    Broader template extension support is planned. See [#465](https://github.com/joshuadavidthomas/django-language-server/issues/465) for details.

## Configuration

Pass additional arguments to `djls check` via the `args` option:

```yaml
repos:
  - repo: https://github.com/joshuadavidthomas/django-language-server
    rev: v6.0.2
    hooks:
      - id: djls-check
        args: [--select, "S100,S117", --color, never]
```

See `djls check --help` for all available options.

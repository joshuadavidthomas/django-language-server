# pre-commit

The dedicated [djls-pre-commit](https://github.com/joshuadavidthomas/djls-pre-commit) repository provides a hook for checking staged Django templates with [`djls check`](cli.md#djls-check). It works with [pre-commit](https://pre-commit.com/) and [prek](https://prek.j178.dev/).

## Usage

Add the hook to `.pre-commit-config.yaml`:

```yaml
repos:
  - repo: https://github.com/joshuadavidthomas/djls-pre-commit
    rev: v6.0.3  # use the latest release tag
    hooks:
      - id: djls-check
```

The hook tag pins and installs the matching django-language-server release. You do not need a separate system installation of `djls`, but your Django project still needs a supported Python environment that the checker can discover.

The hook runs once with all staged `.html`, `.htm`, and `.djhtml` files. It checks templates and does not format or rewrite them.

## Configuration

Pass [`djls check` options](cli.md#options) through `args`:

```yaml
repos:
  - repo: https://github.com/joshuadavidthomas/djls-pre-commit
    rev: v6.0.3
    hooks:
      - id: djls-check
        args: [--select, "S100,S117", --color, never]
```

Project settings in `djls.toml`, `.djls.toml`, or `pyproject.toml` apply to hook runs in the same way as direct CLI runs.

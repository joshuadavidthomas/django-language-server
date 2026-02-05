# Template Linter

Static validation for Django template tag and filter syntax by extracting rules from Django's source code. This project avoids runtime validation and instead uses AST analysis and tokenization to validate templates offline.

**Goals**
- Validate Django template tag syntax statically, without executing Django runtime code.
- Validate filter argument counts using extracted filter signatures.
- Provide coverage diagnostics for Django's built-in tags and filters.
- Keep a lightweight, standalone package that can be reused outside the Django repo.

**Layout**
- `src/template_linter/`: library implementation
- `tests/`: pytest suite

**Running Tests**
From the repo root:
1. `just -f template_linter/Justfile sync`
2. `just -f template_linter/Justfile test`

Other useful commands:
- `just -f template_linter/Justfile format`
- `just -f template_linter/Justfile lint`
- `just -f template_linter/Justfile typecheck`
- `just -f template_linter/Justfile corpus-sync`
- `just -f template_linter/Justfile corpus-test`

**Notes**
- The test suite expects Django’s source tree at `../django` relative to `template_linter/`.
- Dependencies are managed with `uv` via `template_linter/pyproject.toml`.

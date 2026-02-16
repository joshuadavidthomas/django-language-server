# djls-dev

Development tools for the django-language-server workspace.

## cargo-stow

Workspace linting and structure validation, vendored from [BoundaryML's BAML project](https://github.com/BoundaryML/baml) (`baml_language/crates/tools_stow`).

The upstream tool assumes underscore-separated crate names (`myapp_core`). This vendored copy patches namespace detection to also support hyphen-separated names (`djls-core`), matching the naming convention used in this workspace.

### Usage

```bash
just stow          # Validate workspace structure and deps
just stow-fix      # Auto-fix dependency sorting
just architecture  # Generate architecture diagram SVG (requires graphviz)
```

Or directly:

```bash
cargo run -p djls-dev --bin cargo-stow -- stow
cargo run -p djls-dev --bin cargo-stow -- stow --fix
cargo run -p djls-dev --bin cargo-stow -- stow --graph architecture/architecture.svg
```

### What it validates

- Crate names follow the `djls-*` namespace convention
- All dependencies use `{ workspace = true }` format
- Dependencies are sorted: internal deps first (alphabetically), then external deps (alphabetically), separated by a blank line
- No nested crates (flat `crates/` layout only)
- Crate folder names match package names
- Dependency restriction rules (configurable in `stow.toml`)

### Configuration

See [`stow.toml`](../../stow.toml) at the workspace root.

## Attribution

`cargo-stow` is originally authored by [BoundaryML](https://github.com/BoundaryML) as part of the [BAML](https://github.com/BoundaryML/baml) project, licensed under MIT. The vendored source is at `src/cargo_stow.rs`.

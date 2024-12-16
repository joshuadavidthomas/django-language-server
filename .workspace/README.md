# django-language-server Workspace

This is an empty Python package that exists solely to satisfy Hatch's package requirements while using [uv workspaces](https://docs.astral.sh/uv/concepts/projects/workspaces/).

The actual packages are located in:

- [`crates/`](../crates/) - Rust implementation of the LSP server
- [`packages/djls-agent/`](../packages/djls-agent/) - Django introspection agent
- [`packages/djls-server/`](../packages/djls-server/) - Python package that distributes the LSP server binary

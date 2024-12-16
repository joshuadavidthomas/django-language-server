set unstable := true

justfile := justfile_directory() + "/.just/docs.just"
mkdoc_config := justfile_directory() + "/.mkdocs.yml"

[private]
default:
    @just --list --justfile {{ justfile }}

[private]
fmt:
    @just --fmt --justfile {{ justfile }}

# Build documentation
[no-cd]
build LOCATION="site": process
    uv run --group docs --frozen mkdocs build --config-file {{ mkdoc_config }} --site-dir {{ LOCATION }}

# Serve documentation locally
[no-cd]
serve PORT="8000": process
    #!/usr/bin/env sh
    HOST="localhost"
    if [ -f "/.dockerenv" ]; then
        HOST="0.0.0.0"
    fi
    uv run --group docs --frozen mkdocs serve --config-file {{ mkdoc_config }} --dev-addr localhost:{{ PORT }}

[no-cd]
[private]
process:
    uv run docs/processor.py

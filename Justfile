set dotenv-load := true
set unstable := true

mod docs ".just/docs.just"

# List all available commands
[private]
default:
    @just --list

bumpver *ARGS:
    uv run --with bumpver bumpver {{ ARGS }}

clean:
    rm -rf target/

# run pre-commit on all files
lint:
    @just --fmt
    uv run --with pre-commit-uv pre-commit run --all-files

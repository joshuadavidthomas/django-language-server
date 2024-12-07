set dotenv-load := true
set unstable := true

# List all available commands
[private]
default:
    @just --list

# run pre-commit on all files
lint:
    @just --fmt
    uv run --with pre-commit-uv pre-commit run --all-files

set dotenv-load := true
set unstable := true

mod proto ".just/proto.just"

# List all available commands
[private]
default:
    @just --list

clean:
    rm -rf target/

# run pre-commit on all files
lint:
    @just --fmt
    uv run --with pre-commit-uv pre-commit run --all-files

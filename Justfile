set dotenv-load := true
set unstable := true

mod dev ".just/devtools.just"
mod docs ".just/docs.just"

# List all available commands
[private]
default:
    @just --list --list-submodules

[private]
cog:
    uv run --with cogapp --with nox --no-project cog -r CONTRIBUTING.md README.md pyproject.toml

[private]
nox SESSION *ARGS:
    uv run nox --session "{{ SESSION }}" -- "{{ ARGS }}"

bumpver *ARGS:
    uv run --with bumpver bumpver {{ ARGS }}

check *ARGS:
    cargo check {{ ARGS }}

clean:
    cargo clean

clippy:
    cargo clippy --all-targets --all-features --fix -- -D warnings

fmt *ARGS:
    cargo +nightly fmt {{ ARGS }}

# run pre-commit on all files
lint:
    @just --fmt
    @just nox lint

test *ARGS:
    @just nox test {{ ARGS }}

testall *ARGS:
    @just nox tests {{ ARGS }}

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
    uv run --no-project --with cogapp --with nox cog -r CONTRIBUTING.md README.md docs/versioning.md pyproject.toml

[private]
nox SESSION *ARGS:
    uv run --no-project --with "nox[uv]" nox --session "{{ SESSION }}" -- "{{ ARGS }}"

bumpver *ARGS:
    uv run --with bumpver bumpver {{ ARGS }}

check *ARGS:
    cargo check {{ ARGS }}

clean:
    cargo clean

corpus *ARGS:
    cargo run -q -p djls-corpus -- {{ ARGS }}

clippy *ARGS:
    cargo clippy --all-targets --all-features --benches --fix {{ ARGS }} -- -D warnings

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

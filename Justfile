set dotenv-load
set unstable

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
    cargo run -q -p djls-testing --bin corpus -- {{ ARGS }}

clippy *ARGS:
    cargo clippy --all-targets --all-features --benches --fix {{ ARGS }} -- -D warnings

hawk *ARGS:
    @# Work around astral-sh/hawk#74: Hawk's rustc probe can poison Cargo's
    @# target/.rustc_info.json cache, so isolate Hawk's Cargo target/cache.
    CARGO_TARGET_DIR=target/hawk cargo +1.95.0 hawk {{ ARGS }}

e2e *ARGS:
    uv run nox -s e2e -- "{{ ARGS }}"

fixtures *ARGS:
    uv run nox -s fixtures -- "{{ ARGS }}"

fmt *ARGS:
    cargo +nightly fmt {{ ARGS }}

# run pre-commit on all files
lint *ARGS:
    @just --fmt
    @just nox lint {{ ARGS }}

run *ARGS:
    cargo run -p djls -- {{ ARGS }}

test *ARGS:
    @just nox test {{ ARGS }}

testall *ARGS:
    @just nox tests {{ ARGS }}

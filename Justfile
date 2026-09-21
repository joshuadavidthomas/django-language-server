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

# Run corpus setup and tests, or forward subcommands to the management CLI
[positional-arguments]
corpus *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "$#" -eq 0 ]; then
        just nox corpus
    else
        cargo run -q -p djls-testing --bin corpus -- "$@"
    fi

clippy *ARGS:
    cargo clippy --all-targets --all-features --benches --fix --allow-dirty {{ ARGS }} -- -D warnings

# cargo-hawk must run on the toolchain it was built against.
# Keep this paired with the Hawk version in mise.toml.
hawk_channel := `sed -n 's/^channel = "\([^"]*\)"/\1/p' tools/hawk/rust-toolchain.toml`

[positional-arguments]
hawk *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo "+{{ hawk_channel }}" hawk check \
        --manifest-path "{{ justfile_directory() }}/Cargo.toml" \
        --target-dir "{{ justfile_directory() }}/target/hawk" \
        -D warnings "$@"

e2e *ARGS:
    @just nox e2e {{ ARGS }}

fixtures *ARGS:
    @just nox fixtures {{ ARGS }}

rustfmt_channel := `sed -n 's/^channel = "\([^"]*\)"/\1/p' tools/rustfmt/rust-toolchain.toml`

fmt *ARGS:
    cargo "+{{ rustfmt_channel }}" fmt --manifest-path "{{ justfile_directory() }}/Cargo.toml" --all {{ ARGS }}

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

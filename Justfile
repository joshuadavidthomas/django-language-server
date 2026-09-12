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
    cargo clippy --all-targets --all-features --benches --fix --allow-dirty {{ ARGS }} -- -D warnings

# cargo-hawk must run on the toolchain it was built against.
# Bump this when a new cargo-hawk release requires a newer Rust (CI checks latest).
hawk_channel := `sed -n 's/^channel = "\([^"]*\)"/\1/p' tools/hawk/rust-toolchain.toml`

hawk *ARGS:
    cargo "+{{ hawk_channel }}" hawk check \
        --manifest-path "{{ justfile_directory() }}/Cargo.toml" \
        --target-dir "{{ justfile_directory() }}/target/hawk" \
        -D warnings {{ ARGS }}

e2e *ARGS:
    @just nox e2e {{ ARGS }}

fixtures *ARGS:
    @just nox fixtures {{ ARGS }}

fmt *ARGS:
    cd tools/rustfmt && cargo fmt --manifest-path "{{ justfile_directory() }}/Cargo.toml" --all {{ ARGS }}

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

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

# run ast-grep scan and rule tests
ast-grep:
    sg scan --config tools/ast-grep/sgconfig.yml --report-style=short --color=never
    sg test --config tools/ast-grep/sgconfig.yml --skip-snapshot-tests --color=never

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

hawk *ARGS:
    @# Avoid astral-sh/hawk#74 rustc-info cache poisoning.
    @# Keep Hawk focused on visibility; clippy owns dead-code and unused checks.
    RUSTFLAGS="${RUSTFLAGS:-} -A dead_code -A unused_imports" CARGO_CACHE_RUSTC_INFO=0 cargo +1.95.0 hawk --target-dir target/hawk {{ ARGS }}

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

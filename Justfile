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

[positional-arguments]
[private]
nox SESSION *ARGS:
    uv run --no-project --with "nox[uv]" nox --session "$@"

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

# Pass `--fix --allow-dirty` to apply suggestions
[positional-arguments]
clippy *ARGS:
    cargo clippy --all-targets --all-features --benches "$@" -- -D warnings

# Needs `just corpus sync --source-only`. ARGS go to libFuzzer, such as `-max_total_time=600`.
# Fuzz the template parser, seeded from corpus templates
[positional-arguments]
fuzz *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    seeds=fuzz/corpus/parse_template
    mkdir -p "$seeds"
    find crates/djls-testing/.corpus/repos -type f \( -name '*.html' -o -name '*.htm' -o -name '*.djhtml' \) -size -64k -print0 \
        | while IFS= read -r -d '' file; do cp "$file" "$seeds/$(sha1sum < "$file" | cut -c1-40)"; done
    cargo "+{{ rustfmt_channel }}" fuzz run --fuzz-dir fuzz parse_template "$seeds" -- -max_len=65536 "$@"

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
        --exclude-crate djls_wasm \
        -D warnings "$@"

# Run `djls check` from the base and current builds across the corpus and diff the diagnostics
ecosystem BASE="origin/main":
    #!/usr/bin/env bash
    set -euo pipefail
    out="target/ecosystem"
    rm -rf "$out/base-src" && mkdir -p "$out"
    git worktree add --detach "$out/base-src" "{{ BASE }}" >/dev/null
    trap 'git worktree remove --force "$out/base-src"' EXIT
    cargo build --release -q -p djls --manifest-path "$out/base-src/Cargo.toml" --target-dir "$out/base-target"
    cargo build --release -q -p djls
    uv run tools/ecosystem.py run --djls "$out/base-target/release/djls" --output "$out/base.json"
    uv run tools/ecosystem.py run --djls target/release/djls --output "$out/head.json"
    uv run tools/ecosystem.py diff "$out/base.json" "$out/head.json"

# Mutation-test a crate or file, e.g. `just mutants -p djls-semantic -f crates/djls-semantic/src/scoping/loads.rs`
[positional-arguments]
mutants *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    skips=()
    while read -r filter; do skips+=(--skip "$filter"); done < <(grep -v '^#' tools/corpus-sweep-filters.txt)
    cargo mutants --in-place "$@" -- -- "${skips[@]}"

[positional-arguments]
e2e *ARGS:
    @just nox e2e -- "$@"

[positional-arguments]
fixtures *ARGS:
    @just nox fixtures -- "$@"

rustfmt_channel := `sed -n 's/^channel = "\([^"]*\)"/\1/p' tools/rustfmt/rust-toolchain.toml`

fmt *ARGS:
    cargo "+{{ rustfmt_channel }}" fmt --manifest-path "{{ justfile_directory() }}/Cargo.toml" --all {{ ARGS }}

# Check Justfile formatting and run pre-commit on all files
[positional-arguments]
lint *ARGS:
    @just --fmt --check
    @just nox lint -- "$@"

run *ARGS:
    cargo run -p djls -- {{ ARGS }}

[positional-arguments]
test *ARGS:
    @just nox test -- "$@"

# Run one cell of the Python/Django test matrix, e.g. `just test-cell 3.12 6.0`
[positional-arguments]
test-cell PYTHON DJANGO *ARGS:
    @python="$1" django="$2"; shift 2; just nox "tests(python='$python', django='$django')" -- "$@"

[positional-arguments]
testall *ARGS:
    @just nox tests -- "$@"

# Build the browser package into OUT: wasm-bindgen `--target web` output plus each vendored Django
# release re-zipped uncompressed, which compresses better over HTTP and loads without inflating
wasm OUT="target/wasm/pkg":
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build -p djls-wasm --target wasm32-unknown-unknown --profile wasm-release
    rm -rf "{{ OUT }}" && mkdir -p "{{ OUT }}"
    wasm-bindgen --target web --out-dir "{{ OUT }}" target/wasm32-unknown-unknown/wasm-release/djls_wasm.wasm
    for archive in crates/djls-project/vendor/django-*.zip; do
        python3 -I -c 'import sys, zipfile
    with zipfile.ZipFile(sys.argv[1]) as src, zipfile.ZipFile(sys.argv[2], "w", zipfile.ZIP_STORED) as dst:
        for info in src.infolist():
            dst.writestr(info.filename, src.read(info))' "$archive" "{{ OUT }}/$(basename "$archive")"
    done

set unstable

justfile := justfile_directory() + "/.just/playground.just"
root := justfile_directory()
playground := root + "/playground"
wasm_out := playground + "/djls-wasm"

[private]
default:
    @just --list --justfile {{ justfile }}

[private]
fmt:
    @just --fmt --justfile {{ justfile }}

# Build djls-wasm and generate its JS bindings into playground/djls-wasm
wasm:
    #!/usr/bin/env bash
    set -euo pipefail
    cd "{{ root }}"
    # The CLI must match the wasm-bindgen crate exactly; mise.toml pins it.
    locked=$(sed -n '/^name = "wasm-bindgen"$/{n;s/^version = "\(.*\)"$/\1/p;}' Cargo.lock)
    installed=$(wasm-bindgen --version 2>/dev/null | cut -d' ' -f2 || true)
    if [ "$installed" != "$locked" ]; then
        echo "wasm-bindgen-cli $locked is required (found '${installed:-none}'). Run 'mise install' or 'cargo install wasm-bindgen-cli --version $locked --locked'." >&2
        exit 1
    fi
    rustup --quiet target add wasm32-unknown-unknown
    cargo build -p djls-wasm --target wasm32-unknown-unknown --release
    wasm-bindgen --target web --out-dir "{{ wasm_out }}" \
        target/wasm32-unknown-unknown/release/djls_wasm.wasm

[private]
install:
    #!/usr/bin/env bash
    set -euo pipefail
    cd "{{ playground }}"
    if [ ! node_modules/.package-lock.json -nt package-lock.json ]; then
        npm ci
    fi

# Run the playground dev server; run `just playground wasm` first
[positional-arguments]
dev *ARGS: install
    #!/usr/bin/env bash
    set -euo pipefail
    if [ ! -f "{{ wasm_out }}/djls_wasm.js" ]; then
        echo "Missing {{ wasm_out }}; run 'just playground wasm' first." >&2
        exit 1
    fi
    cd "{{ playground }}"
    npm run dev -- "$@"

# Type-check the playground
check: install
    cd "{{ playground }}" && npm run check

# Build the static playground into playground/dist
build: wasm install
    cd "{{ playground }}" && npm run build

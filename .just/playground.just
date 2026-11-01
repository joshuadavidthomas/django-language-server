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

# Build djls-wasm, its JS bindings, and the Django sources it loads into playground/djls-wasm
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
    # No wasm-opt: it shrinks the raw module but grows it after gzip/brotli (Cargo.toml).
    cargo build -p djls-wasm --target wasm32-unknown-unknown --profile wasm-release
    wasm-bindgen --target web --out-dir "{{ wasm_out }}" \
        target/wasm32-unknown-unknown/wasm-release/djls_wasm.wasm
    # The wasm embeds no Django; the worker fetches one release and registers it (DJANGO in
    # src/lib/project.ts picks which). Stored rather than deflated, so HTTP compression does the
    # work: 5.2 is about 1 MB brotli, against 1.6 MB for the deflated zip.
    for zip in crates/djls-project/vendor/django-*.zip; do
        python3 -c 'import sys, zipfile; src, dst = sys.argv[1:]; z = zipfile.ZipFile(src); out = zipfile.ZipFile(dst, "w", zipfile.ZIP_STORED); [out.writestr(i, z.read(i), zipfile.ZIP_STORED) for i in z.infolist()]; out.close()' \
            "$zip" "{{ wasm_out }}/$(basename "$zip")"
    done

[private]
install:
    cd "{{ playground }}" && pnpm install --frozen-lockfile

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
    pnpm dev "$@"

# Type-check the playground
check: install
    cd "{{ playground }}" && pnpm check

# Check formatting and lint the playground (oxfmt, oxlint, eslint)
lint: install
    cd "{{ playground }}" && pnpm lint

# Format the playground
format: install
    cd "{{ playground }}" && pnpm format

# Build the static playground into playground/build
build: wasm install
    cd "{{ playground }}" && pnpm build

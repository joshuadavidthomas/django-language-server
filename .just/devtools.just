set unstable := true

justfile := justfile_directory() + "/.just/devtools.just"

[private]
default:
    @just --list --justfile {{ justfile }}

[private]
[no-cd]
datasette *ARGS:
    uvx --with datasette-pretty-json datasette {{ ARGS }}

[private]
[no-cd]
devtools *ARGS:
    uvx lsp-devtools {{ ARGS }}

[private]
fmt:
    @just --fmt --justfile {{ justfile }}

[no-cd]
debug:
    cargo run --bin djls-tmux --package djls-dev

[no-cd]
explore FILENAME="djls.db":
    @just dev datasette -o {{ FILENAME }}

[no-cd]
inspect:
    @just dev devtools inspect

[no-cd]
record FILENAME="djls.db":
    rm -f {{ FILENAME }}
    @just dev devtools record --to-sqlite {{ FILENAME }}

# Profile a bench with valgrind-codspeed's callgrind (same tool CI uses).
# Produces deterministic per-function instruction counts with call trees.
# Harness overhead is automatically stripped by the codspeed fork.
# Requires: valgrind-codspeed (https://github.com/CodSpeedHQ/valgrind-codspeed), jq, rg
# Usage: just dev profile <bench> [filter]
# Example: just dev profile diagnostics collect_diagnostics_realistic
[no-cd]
profile bench filter="":
    #!/usr/bin/env bash
    set -euo pipefail

    for cmd in valgrind jq rg; do
        if ! command -v "$cmd" &>/dev/null; then
            echo "Error: required command '$cmd' not found" >&2
            exit 1
        fi
    done

    if ! valgrind --version 2>/dev/null | rg -q 'codspeed'; then
        echo "Error: requires valgrind-codspeed fork (https://github.com/CodSpeedHQ/valgrind-codspeed)" >&2
        echo "  Install: git clone --depth 1 https://github.com/CodSpeedHQ/valgrind-codspeed /tmp/valgrind-codspeed" >&2
        echo "           cd /tmp/valgrind-codspeed && ./autogen.sh && ./configure --prefix=\$HOME/.local && make -j\$(nproc) && make install" >&2
        exit 1
    fi

    bench="{{ bench }}"
    filter="{{ filter }}"
    profiles_dir="profiles"

    label="${bench}"
    if [ -n "$filter" ]; then
        label="${bench}-${filter}"
    fi

    mkdir -p "$profiles_dir"

    echo "Building bench '${bench}' with debug info..."
    cargo bench -p djls-bench --bench "$bench" --no-run -q

    binary=$(cargo bench -p djls-bench --bench "$bench" --no-run --message-format=json 2>/dev/null \
        | jq -r 'select(.executable != null and .target.name == "'"$bench"'") | .executable' \
        | head -1)

    if [ -z "$binary" ] || [ ! -f "$binary" ]; then
        echo "Error: could not find bench binary for '${bench}'" >&2
        exit 1
    fi

    outfile="${profiles_dir}/${label}.callgrind"

    echo "Profiling with callgrind..."
    filter_args=""
    if [ -n "$filter" ]; then
        filter_args="$filter"
    fi

    # shellcheck disable=SC2086
    valgrind --tool=callgrind \
        --callgrind-out-file="$outfile" \
        --cache-sim=yes \
        --collect-jumps=yes \
        "$binary" $filter_args 2>&1 \
        | rg -v '^==' || true

    # codspeed's valgrind fork writes two files: the base filename has
    # benchmark-only data (harness overhead stripped), .1 has everything.
    rm -f "${outfile}."*

    echo ""
    echo "=== Hot functions (self cost) ==="
    echo ""
    callgrind_annotate --auto=yes "$outfile" \
        | rg -v '^\s*\.' \
        | head -60 || true

    echo ""
    echo "=== Call tree (inclusive cost) ==="
    echo ""
    callgrind_annotate --inclusive=yes --tree=calling "$outfile" \
        | rg -v '^\s*\.' \
        | head -120 || true

    echo ""
    echo "Callgrind data: ${outfile}"

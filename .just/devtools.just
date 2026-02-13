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
# Produces per-function instruction counts — deterministic, not statistical.
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
    # clean up the harness dump.
    rm -f "${outfile}."*

    echo ""
    echo "Annotating top functions..."
    callgrind_annotate --auto=yes "$outfile" \
        | rg -v '^\s*\.' \
        | head -80 || true

    echo ""
    echo "Output:"
    echo "  Callgrind data: ${outfile}"
    echo ""
    echo "Inspect with:"
    echo "  callgrind_annotate ${outfile}          # text summary"
    echo "  kcachegrind ${outfile}                  # GUI (if installed)"

# Profile a bench with perf sampling + flamegraph (fallback if valgrind unavailable).
# Requires: jq, flamegraph, rg
# Usage: just dev flamegraph <bench> [filter]
[no-cd]
flamegraph bench filter="":
    #!/usr/bin/env bash
    set -euo pipefail

    for cmd in jq flamegraph rg; do
        if ! command -v "$cmd" &>/dev/null; then
            echo "Error: required command '$cmd' not found" >&2
            exit 1
        fi
    done

    bench="{{ bench }}"
    filter="{{ filter }}"
    profiles_dir="profiles"

    label="${bench}"
    if [ -n "$filter" ]; then
        label="${bench}-${filter}"
    fi

    mkdir -p "$profiles_dir"

    echo "Building bench '${bench}' with debug info + frame pointers..."
    RUSTFLAGS="-C force-frame-pointers=yes" \
        cargo bench -p djls-bench --bench "$bench" --no-run -q

    binary=$(RUSTFLAGS="-C force-frame-pointers=yes" \
        cargo bench -p djls-bench --bench "$bench" --no-run --message-format=json 2>/dev/null \
        | jq -r 'select(.executable != null and .target.name == "'"$bench"'") | .executable' \
        | head -1)

    if [ -z "$binary" ] || [ ! -f "$binary" ]; then
        echo "Error: could not find bench binary for '${bench}'" >&2
        exit 1
    fi

    echo "Profiling with perf..."
    filter_args=""
    if [ -n "$filter" ]; then
        filter_args="$filter"
    fi

    # shellcheck disable=SC2086
    flamegraph -o "${profiles_dir}/${label}.svg" \
        --post-process "tee ${profiles_dir}/${label}.collapsed" \
        -F 9999 \
        -- "$binary" $filter_args 2>&1 \
        | rg -v 'cmd__addr2line'

    echo ""
    echo "Output:"
    echo "  Collapsed stacks: ${profiles_dir}/${label}.collapsed"
    echo "  Flamegraph SVG:   ${profiles_dir}/${label}.svg"

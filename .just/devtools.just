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

# Profile a bench and generate a flamegraph SVG + collapsed stacks.
# Requires: jq, flamegraph, rg
# Usage: just dev profile <bench> [filter]
# Example: just dev profile parser parse_template
[no-cd]
profile bench filter="":
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

    echo "Building bench '${bench}' with debug info..."
    cargo bench -p djls-bench --bench "$bench" --no-run -q

    binary=$(cargo bench -p djls-bench --bench "$bench" --no-run --message-format=json 2>/dev/null \
        | jq -r 'select(.executable != null and .target.name == "'"$bench"'") | .executable' \
        | head -1)

    if [ -z "$binary" ] || [ ! -f "$binary" ]; then
        echo "Error: could not find bench binary for '${bench}'" >&2
        exit 1
    fi

    echo "Profiling..."
    filter_args=""
    if [ -n "$filter" ]; then
        filter_args="$filter"
    fi

    # shellcheck disable=SC2086
    flamegraph -o "${profiles_dir}/${label}.svg" \
        --post-process "tee ${profiles_dir}/${label}.collapsed" \
        -- "$binary" $filter_args 2>&1 \
        | rg -v 'cmd__addr2line'

    echo ""
    echo "Output:"
    echo "  Collapsed stacks: ${profiles_dir}/${label}.collapsed"
    echo "  Flamegraph SVG:   ${profiles_dir}/${label}.svg"

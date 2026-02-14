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

# TODO: djls-tmux binary was removed in #214, this recipe needs updating
[no-cd]
debug:
    @echo "Error: djls-tmux was removed in PR #214. This recipe needs updating." >&2 && exit 1

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

# Profile a bench with callgrind
[no-cd]
profile bench filter="":
    #!/usr/bin/env bash
    set -euo pipefail

    for cmd in valgrind jq rg; do
        if ! command -v "$cmd" &>/dev/null; then
            echo "Error: required command '$cmd' not found" >&2
            echo "See CONTRIBUTING.md for profiling setup instructions." >&2
            exit 1
        fi
    done

    if ! valgrind --version 2>/dev/null | rg -q 'codspeed'; then
        echo "Error: requires the valgrind-codspeed fork, not stock valgrind." >&2
        echo "See CONTRIBUTING.md for install instructions." >&2
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

    # function-level summaries (no source annotation)
    callgrind_annotate --auto=no "$outfile" > "${outfile}.flat.txt" 2>/dev/null
    callgrind_annotate --auto=no --inclusive=yes --tree=calling "$outfile" > "${outfile}.tree.txt" 2>/dev/null

    echo ""
    echo "Hot functions (self cost):"
    echo ""
    # strip the header, show just the function table
    sed -n '/file:function/,$ p' "${outfile}.flat.txt" \
        | rg -v 'codspeed_divan_compat' || true

    echo ""
    echo "Call tree (inclusive cost, who calls what):"
    echo ""
    sed -n '/file:function/,$ p' "${outfile}.tree.txt" \
        | rg -v 'codspeed_divan_compat' || true

    echo ""
    echo "Raw data: ${outfile}"

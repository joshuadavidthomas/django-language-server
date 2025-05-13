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
    #!/usr/bin/env bash
    set -euo pipefail

    tmux has-session -t django-debug 2>/dev/null && tmux kill-session -t django-debug
    pkill -f "lsp-devtools" || true

    tmux new-session -d -s django-debug "nvim tests/project/djls_app/templates/djls_app/base.html"
    tmux split-window -h -p 20 "just dev devtools record"

    tmux select-pane -L

    tmux attach-session -t django-debug

    pkill -f "lsp-devtools" || true

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

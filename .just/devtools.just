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
explore FILENAME="djls.db":
    @just dev datasette -o {{ FILENAME }}

[no-cd]
inspect:
    @just dev devtools inspect

[no-cd]
record FILENAME="djls.db":
    rm -f {{ FILENAME }}
    @just dev devtools record --to-sqlite {{ FILENAME }}

set dotenv-load := true
set unstable := true

mod docs ".just/docs.just"

# List all available commands
[private]
default:
    @just --list

[private]
nox SESSION *ARGS:
    uv run noxfile.py --session "{{ SESSION }}" -- "{{ ARGS }}"

bumpver *ARGS:
    uv run --with bumpver bumpver {{ ARGS }}

clean:
    cargo clean

# run pre-commit on all files
lint:
    @just --fmt
    @just nox lint

test *ARGS:
    @just nox test {{ ARGS }}

testall *ARGS:
    @just nox tests {{ ARGS }}

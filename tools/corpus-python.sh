#!/usr/bin/env bash
# Linux bootstrap for corpus interpreters no longer downloadable through uv.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
interpreters="${1:-$repo_root/crates/djls-testing/.corpus/interpreters}"
if "$interpreters/3.6/bin/python3.6" -c 'import ssl, ctypes' 2>/dev/null &&
   "$interpreters/3.7/bin/python3.7" -c 'import ssl, ctypes' 2>/dev/null; then
    exit 0
fi

# python-build carries the compiler compatibility patches for these EOL releases.
pyenv_ref=3787bacc9188d76ba7ca24c23e26afb1a841a543
source_dir="$(mktemp -d)"
trap 'rm -rf "$source_dir"' EXIT
curl --proto '=https' --tlsv1.2 -fsSL \
    "https://github.com/pyenv/pyenv/archive/$pyenv_ref.tar.gz" \
    -o "$source_dir/pyenv.tar.gz"
tar -xzf "$source_dir/pyenv.tar.gz" --strip-components=1 -C "$source_dir"
mkdir -p "$interpreters"

for version in 3.6.15 3.7.17; do
    minor="${version%.*}"
    if ! "$interpreters/$minor/bin/python$minor" -c 'import ssl, ctypes' 2>/dev/null; then
        # Old CPython's ctypes code can crash with modern compiler optimizations.
        CFLAGS=-O0 MAKE_OPTS=-j4 \
            "$source_dir/plugins/python-build/bin/python-build" \
            "$version" "$interpreters/$minor"
    fi
    "$interpreters/$minor/bin/python$minor" -c 'import ssl, ctypes; print(ssl.OPENSSL_VERSION)'
done

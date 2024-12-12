set unstable := true

justfile := justfile_directory() + "/.just/proto.just"

[private]
default:
    @just --list --justfile {{ justfile }}

[no-cd]
[private]
check:
    #!/usr/bin/env sh
    if ! command -v protoc > /dev/null 2>&1; then
        echo "protoc is not installed. Please install protobuf-compiler"
        exit 1
    fi

[private]
fmt:
    @just --fmt --justfile {{ justfile }}

# Generate protobuf code for both Rust and Python
[no-cd]
gen:
  @just proto rust
  @just proto py

# Generate protobuf code for Rust
[no-cd]
rust: check
    cargo build -p djls-types

# Generate protobuf code for Python
[no-cd]
py: check
    protoc -I=proto --python_out=python/djls proto/*.proto


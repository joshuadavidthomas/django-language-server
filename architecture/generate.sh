#!/bin/bash

set -e

cd "$(dirname "$0")/.."

cargo run -p djls-dev --bin cargo-stow -- stow --graph architecture/architecture.svg

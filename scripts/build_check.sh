#!/usr/bin/env bash
# Lumecast contracts: local build + check pipeline (mirrors CI).
set -euo pipefail

cd "$(dirname "$0")/.."

echo "==> fmt"
cargo fmt --all --check

echo "==> clippy (-D warnings)"
cargo clippy --workspace --all-targets -- -D warnings

echo "==> tests"
cargo test --workspace

echo "==> wasm (debug once, via stellar-cli so SDK 28 spec-shaking is applied)"
stellar contract build --package lumecast-market --optimize=false

echo "==> all green"
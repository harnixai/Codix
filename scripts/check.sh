#!/usr/bin/env bash
# 一批改动做完跑一次：格式、clippy、全部测试。
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

#!/usr/bin/env bash
# 一批改动做完跑一次：格式、clippy、全部测试。
set -euo pipefail
cd "$(dirname "$0")/.."
# 不带 --all：--all 连本地路径依赖（3rds 里拷来的、不改的流程库）也格式化。
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

#!/usr/bin/env bash
# 按 3rds/INDEX.md 把第三方代码拷到 3rds/：原样整目录拷入，本地不改。
set -euo pipefail
cd "$(dirname "$0")/.."
HARNIX_DIR="${HARNIX_DIR:-../Harnix-next}"
HARNIX_REV="4b7111b8"

have=$(git -C "$HARNIX_DIR" rev-parse --short=8 HEAD)
if ! git -C "$HARNIX_DIR" merge-base --is-ancestor "$HARNIX_REV" HEAD; then
  echo "$HARNIX_DIR 在 ${have}，不含 ${HARNIX_REV}" >&2
  exit 1
fi
if ! git -C "$HARNIX_DIR" diff --quiet "$HARNIX_REV" HEAD -- crates/flow \
  || [ -n "$(git -C "$HARNIX_DIR" status --porcelain -- crates/flow)" ]; then
  echo "$HARNIX_DIR 的 crates/flow 与 ${HARNIX_REV} 不同；先换到那个版本，或更新 INDEX.md 与本脚本" >&2
  exit 1
fi
rm -rf 3rds/harnix-flow
cp -R "$HARNIX_DIR/crates/flow" 3rds/harnix-flow
rm -f 3rds/harnix-flow/Cargo.lock
echo "3rds/harnix-flow ← $HARNIX_DIR@${have}（crates/flow 同 ${HARNIX_REV}）"

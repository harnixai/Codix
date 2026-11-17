#!/usr/bin/env bash
# LIVE 测试（testing.md §8）：真实的实例进程、真实的 Agix 与模型，经 TUI 那条接入路径测 Codix
# 自己的功能。凭据取仓库根的 .env（CODIX_LIVE_ENV 可指定别的文件）。
#
#   scripts/live.sh                    全部 live 用例
#   scripts/live.sh live_jobs          只跑一个文件（live_instance / live_attach / live_sessions / live_turns / live_jobs / live_tools / live_flows）
#   scripts/live.sh live_jobs catch_up 再按用例名过滤
#
# 同时起的实例数用 LIVE_THREADS 控制（缺省 4），太多会撞上模型服务的限流。
set -euo pipefail
cd "$(dirname "$0")/.."
threads="${LIVE_THREADS:-4}"
if [[ $# -ge 1 ]]; then
    file=$1; shift
    exec cargo test -p codix --test "$file" -- --ignored --test-threads="$threads" "$@"
fi
files=()
for f in crates/codix/tests/live_*.rs; do
    files+=(--test "$(basename "$f" .rs)")
done
exec cargo test -p codix "${files[@]}" -- --ignored --test-threads="$threads"

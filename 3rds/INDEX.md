# 3rds 索引

`3rds/` 里放从别的仓库拷进来的代码。**只有这份索引入库**，代码本身不入库，在本地按下表拷到对应目录。

规矩：

- 原样拷入，本地不改。需要改的，到来源仓库提 issue（标签 `codix`），等对方发布后再重新拷。
- 来源有新版本时重新拷一次，同时更新下表的版本列。

| 本地目录 | 来源仓库 | 来源路径 | 版本 | 用途 | 状态 |
|---|---|---|---|---|---|
| `3rds/harnix-flow/` | [openatta/Harnix](https://github.com/openatta/Harnix) | `crates/flow` | `harnix-next` 分支 `4b7111b8`（0.4.5-2026111601，尚未发布） | 流程内核、存储、流程包与派工器 | 已拷入；编写与运行工具待 [Harnix#67](https://github.com/openatta/Harnix/issues/67) |

拷贝用 `scripts/fetch-3rds.sh`：从本地的 Harnix 检出（缺省 `../Harnix-next`，`HARNIX_DIR` 可改）核对版本后整目录拷入。拷进来的 crate 不是 workspace 成员（`exclude`），格式与 clippy 不管它；它自己的测试单独跑：`cargo test --manifest-path 3rds/harnix-flow/Cargo.toml`。

Agix 不在这里：它是 git submodule，位于 `agix/`。

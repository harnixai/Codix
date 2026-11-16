# 3rds 索引

`3rds/` 里放从别的仓库拷进来的代码。**只有这份索引入库**，代码本身不入库，在本地按下表拷到对应目录。

规矩：

- 原样拷入，本地不改。需要改的，到来源仓库提 issue（标签 `codix`），等对方发布后再重新拷。
- 来源有新版本时重新拷一次，同时更新下表的版本列。

| 本地目录 | 来源仓库 | 来源路径 | 版本 | 用途 | 状态 |
|---|---|---|---|---|---|
| `3rds/harnix-flow/` | [openatta/Harnix](https://github.com/openatta/Harnix) | `crates/flow`（拆分前在 `crates/ext/flow`） | — | 流程内核与 runner | 等 [Harnix#66](https://github.com/openatta/Harnix/issues/66) 拆分完成 |

Agix 不在这里：它是 git submodule，位于 `agix/`。

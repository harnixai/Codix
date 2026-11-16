# Codix 测试方案

阶段 2 的目标是：只用 mock 后端，把 `interaction.md` 里的每一条交互都验证到。阶段 3 接缝时，这一套测试保持不动，再加上针对真实后端的用例。

## 1. 分层

| 层 | 测什么 | 怎么测 | 需要终端吗 |
|---|---|---|---|
| L1 单元 | `model::update` 的状态变化与产出的 Effect；转录排版（宽度、CJK、`\n`、折叠） | 直接构造 `State`，喂 `Msg`，断言结果 | 不需要 |
| L2 投影快照 | `model::view` → `tui::render` 画出来的画面 | 渲染到 ratatui 的 `TestBackend`，导出为文本快照，与存档比较 | 不需要 |
| L3 场景 | 一段完整的交互：mock 推事件 + 一串按键 → 在若干检查点比对快照或断言状态 | 无终端的事件循环 + `ScriptedBackend` + tokio 暂停时钟 | 不需要 |
| L4 守卫 | 键位表没有冲突、每个动作都有帮助文字；每个 `FrameState` 字段至少被一张快照覆盖；crate 依赖方向正确 | 普通测试 | 不需要 |
| L5 真终端 | 键盘协议、颜色、宽字符、resize、粘贴、tmux 和 ssh 下的表现 | 人工照 `docs/manual-smoke-checklist.md` 过一遍（阶段 2 编写） | 需要 |

L1–L4 全部离线，不需要 API key，在 CI 里跑。

**不写计时断言。** 性能用**计数**来验，不用耗时，因为计时在高负载下会抖：

- 一个 `TextDelta` 只重排 1 个块
- 一次排空消息队列最多画 1 帧
- 没有动画时，Tick 计时器处于关闭状态

## 2. 无终端的事件循环

`app` 的循环要拆成两部分：

- `Core`：持有 `State`、按键解析、后端句柄、时钟，提供 `step(input) -> Vec<Effect>` 和 `frame(size) -> FrameState`。
- 外壳：真实终端的读写，只在 `main` 里使用。

L3 驱动的是 `Core`：

```rust
let mut h = Harness::new(Scenario::load("m04-prompts"), Size::new(120, 40));
h.advance(ms(300));                     // 让场景时间线走 300ms
h.keys("Ctrl+G");                       // 进入提问框
h.snapshot("m04-ask-open");             // 与 tests/snapshots/m04-ask-open.txt 比较
h.keys("2 Enter");
assert_eq!(h.backend().calls().last(), Some(&Command::Answer { .. }));
```

快照只记字符，不记样式。样式（高亮、颜色）需要验证时，在 L1 里断言 `FrameState` 上对应的字段。

快照的尺寸固定为三档：`80×24`、`120×40`、`200×50`。设置环境变量 `CODIX_UPDATE_SNAPSHOTS=1` 时重写快照；重写后的 diff 必须在提交里人工看过。

## 3. mock 后端

### 3.1 种子数据

`crates/mock/seeds/*.yaml`：一组可以复用的初始状态，包括会话（带若干轮历史）、任务树、goal、todo、流程定义（按 Harnix 的格式手写，方便以后对照）、运行、作业及其历史。

| 种子 | 内容 |
|---|---|
| `empty` | 什么都没有：新用户第一次打开 |
| `default` | 3 个会话（其中一个有 2 层子 Agent 和 4 个任务）、2 个流程（其中一个有 3 次运行）、3 个作业 |
| `busy` | 100 个会话、一个会话下挂 50 个任务、一个流程有 30 次运行：测列表和滚动 |

### 3.2 场景格式

场景文件放在 `crates/mock/scenarios/*.yaml`。其中的事件就是 `port::Event` 的 serde 形式，所以场景文件同时也是端口消息的样例。

```yaml
name: m04-prompts
seed: default
timeline:                       # 场景开始后自动发生的事
  - at: 100
    prompt_pending:
      session: s1
      prompt: {id: p1, kind: permission, tool: Bash, summary: "cargo test", from: {task: t2}}
  - at: 150
    prompt_pending:
      session: s1
      prompt: {id: p2, kind: question, question: "用哪个数据库？", options: ["Postgres", "SQLite"]}
  - at: 5000
    prompt_resolved: {session: s1, id: p1, resolution: {withdrawn: timed_out}}
rules:                          # 命令 → 反应
  - on: {answer: {prompt: p2}}
    reply: ok
    then:
      - {after: 0, prompt_resolved: {session: s1, id: p2, resolution: answered}}
      - {after: 50, play: answer-short}
scripts:                        # 可以复用的事件片段
  answer-short:
    - {after: 0, turn_started: {session: s1}}
    - {after: 30, text_delta: {session: s1, text: "好的，用 SQLite。"}}
    - {after: 30, turn_ended: {session: s1, stop: end_turn, usage: {input: 1200, output: 40}}}
```

- 没有规则匹配的命令，按命令类型给出缺省反应。例如 `Submit` 回放 `default-answer`，`RunOp` 推进运行状态。这样交互运行时随便操作都不会卡住。
- `then` 里的 `after` 相对于命令到达的时刻计算，`timeline` 里的 `at` 相对于场景开始的时刻计算。

### 3.3 交互运行

```sh
codix --mock m11-flow-run            # 用这个场景启动真实终端界面
codix --mock m11-flow-run --speed 4  # 时间线加速 4 倍
codix --mock default                 # 只加载种子，不跑时间线
```

只有 mock 模式下，F12 才会打开**事件日志浮层**：滚动显示后端推来的每个事件和发出的每条命令，人工验收时用来对照"界面是不是如实反映了后端"。

## 4. 场景清单

每个场景对应若干条需求（需求编号）和若干节交互规格（交互 §），阶段 2 都要有 L3 用例。

| 场景 | 内容 | 覆盖 |
|---|---|---|
| m01-chat-basic | 一问一答，流式正文 + 思考 | C-01 C-10 C-11 · 交互 §4.2 §4.3 |
| m02-text-hard | 长 markdown、代码块、CJK、emoji、制表符、带 `\r\n` 的文本、超长单词 | N-04 · 交互 §4.2 |
| m03-tools | 并发工具调用、结果夹在正文中间、折叠阈值、文件修改的 diff | C-02 · 交互 §4.2 |
| m04-prompts | 审批和提问排队、在提问之间切换、选项与自由输入、超时撤回、在别处已答 | C-04 C-05 · 交互 §4.6 §3 |
| m05-interrupt | 流式中途中断；中断后马上再提交 | C-06 |
| m06-step-restart | 同一步重发，丢掉这一步已画的文字；`model_retry` 提示 | C-03 |
| m07-steer | 运行中插话：送达；没赶上被退回、回到输入框 | C-07 · 交互 §4.4 |
| m08-composer | 多行、历史、按词编辑、大段粘贴的占位、slash 补全 | C-08 C-09 · 交互 §4.4 §4.5 |
| m09-session-tree | 3 层子 Agent + 各种任务，状态频繁变化；进入子 Agent、看 Shell 输出、发消息、停止 | S-04 S-05 S-06 C-15 · 交互 §4.1 §5.2 |
| m10-goal-todo | goal 自动开轮、受阻、暂停和恢复；todo 逐项完成 | S-02 S-03 · 交互 §5.2 |
| m11-flow-run | 一次运行走完：并行的站、搁置后放行、等确认、未达标后返工、暂停后恢复、取消 | F-02 F-03 F-04 F-05 · 交互 §5.3 |
| m12-flow-author | 选中流程定义，进入编写会话；新建流程；查看定义 | F-07 F-08 · 交互 §4.1 §5.3 |
| m13-jobs | 简单作业和流程作业按时触发、失败、立即运行、启用和停用、从历史跳转 | J-01 至 J-04 · 交互 §5.4 |
| m14-attention | 多处同时等人（会话、站、受阻的 goal、失败的作业），用 Ctrl+G 依次处理，角标随之变化 | L-04 L-05 |
| m15-reconnect | 断线 → 重连中 → 恢复（`Resynced` + 快照），列表变灰后恢复 | R-04 · 交互 §8 |
| m16-storm | 每秒 1000 个 `TextDelta`，同时 50 个任务在更新：用计数断言重排和画帧的次数 | N-02 |
| m17-layout | 200 / 120 / 100 / 80 / 60 列之间切换；收起右栏；调整比例；单栏下切换 | L-02 L-03 N-01 · 交互 §2 |
| m18-errors | 一轮出错、命令失败、列表加载失败后重试、能力不可用 | 交互 §8 |
| m19-many | `busy` 种子：长列表的滚动、过滤、选中保持 | S-01 F-01 J-01 |
| m20-first-run | `empty` 种子：每个标签的空状态和引导 | 交互 §8 |

## 5. 守卫

| 守卫 | 内容 |
|---|---|
| 依赖方向 | `tui` 不依赖 `port` / `model`；`model` 不依赖 ratatui 的终端后端，也不依赖 tokio 运行时；`port` 不依赖 Agix 和 Harnix；`app` 不依赖 Agix |
| 键位 | 同一上下文里没有重复绑定；每个动作都有帮助文字；每个全局动作在 §6 的表里都有替代键或替代命令 |
| 字段覆盖 | 每个 `FrameState` 字段至少在一张快照里取到非缺省值，防止出现"契约里有、但没人产出"的字段 |
| 块首标记 | 只有 `model` 里那一个辅助函数可以读 `first_of_block`（用 grep 守卫） |

## 6. 阶段 2 的验收标准

1. `interaction.md` 里所有的 P0 交互都能在 mock 下操作出来。
2. 上面 20 个场景的 L3 用例全部通过；`cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` 全部通过。
3. `docs/manual-smoke-checklist.md` 已经写好，并在至少三种环境里过完：一个 macOS 终端、tmux、ssh 到 Linux。
4. 维护者本人用 `codix --mock …` 亲手试过，确认交互没有问题。

四条都满足才进入阶段 3。

# Codix 架构

需求见 `docs/reqs/codix-requirements.md`，交互见 `interaction.md`，测试见 `testing.md`。

## 1. 总览

```
┌──────────────────────────── codix（TUI 进程）──────────────────────────────┐
│                                                                            │
│  终端按键 ──► keys（上下文 + 快捷键 → 动作）                                │
│                 │                                                          │
│                 ▼                                                          │
│  app（事件循环）── Msg ──► model::update(&mut State, Msg) → Vec<Effect>     │
│     ▲   │                         │                                        │
│     │   │                         └─► model::view(&State, Size) → FrameState│
│     │   │                                              │                   │
│     │   │                                              ▼                   │
│     │   │                                   tui::render(FrameState)        │
│     │   ▼ Effect::Call(Command)                                            │
│     │  ┌──────────── port::Backend（唯一端口）────────────┐                │
│     └──┤ Event（领域事件）              Reply（命令结果）  │                │
│        └───────────────┬──────────────────────────────────┘                │
│                        │                                                   │
│          阶段 2：mock::ScriptedBackend    阶段 3：client::HostBackend       │
└────────────────────────┼───────────────────────────────────────────────────┘
                         │ 阶段 3：宿主协议 = port 的消息序列化成 JSON-RPC
                         ▼
              ┌──── host（宿主）──────────────────────────────┐
              │ 流程（3rds/harnix-flow 内核 + runner）         │  本地：同进程，经 duplex
              │ 作业调度                                       │  远程：codix serve，
              │ Agix 协议客户端 ──► Agix                       │        经 unix socket / WebSocket
              └───────────────────────────────────────────────┘
                                   │ Agix JSON-RPC
                                   ▼
                     agix::embed（同进程）或远端 agixd
```

设计要点有三条：

1. **TUI 只认 `port`。** 阶段 2 的 mock 和阶段 3 的真实后端实现的是同一个 trait、同一套消息。接缝就是换一个实现，界面一行不动。
2. **端口的消息本身就是宿主协议。** `Command`、`Event`、`Reply` 都可以序列化，阶段 3 远程化时把它们放上 JSON-RPC 即可，不再另外设计一套协议。
3. **本地和远程走同一条代码路径。** 本地模式下宿主也跑在协议后面（同进程，通过 duplex 通信），这一点照搬 `agix::embed` 的做法。远程只是换了传输。

## 2. crate 划分

| crate | 阶段 | 职责 | 依赖 |
|---|---|---|---|
| `port` | 2 | 后端端口：`Backend` trait；`Command` / `Reply` / `Event`；领域数据类型（会话摘要、任务、goal、todo、流程、运行、站、作业……） | serde, async-trait, tokio（mpsc） |
| `tui` | 2 | 纯渲染：`FrameState` → 画面；`layout::compute`（纯函数，算出各区域的矩形） | ratatui, crossterm, unicode-width |
| `keys` | 2 | 快捷键：解析、带上下文的绑定表、和弦状态机、用户覆盖与冲突校验 | crossterm, serde |
| `model` | 2 | view-model：`State`（领域状态 + 界面状态）、`update`、`view`；转录块的排版缓存 | port, tui（只用 `FrameState` 类型和 `layout`）, keys（只用动作和上下文类型） |
| `mock` | 2 | `ScriptedBackend`：按场景文件推事件、按规则回应命令；种子数据 | port |
| `app` | 2 | 二进制 `codix`：命令行参数、终端生命周期、事件循环、选择后端实现 | 以上全部 |
| `wire` | 3 | Agix 协议的 serde 类型（协议 §4 和 §7）；不认识的 `kind` 一律跳过 | serde |
| `host` | 3 | 宿主：Agix 客户端（rpc + 传输）、Agix 帧 → 领域事件的翻译、流程（`FlowEngine` 实现）、作业调度；对外暴露端口消息 | port, wire, agix, 3rds/harnix-flow |
| `client` | 3 | `HostBackend`：通过传输（duplex / unix / ws）连接宿主，实现 `Backend` | port |

**依赖边界**（阶段 2 用测试守住）：

- `tui` 不依赖 `port`、`model`，更不依赖 Agix。它只知道 `FrameState`。
- `model` 不依赖 ratatui 的渲染、不碰 IO、不起任务：`update` 和 `view` 都是同步的纯函数，所以可以完全离线测试。
- `port` 不依赖任何 Agix 或 Harnix 类型。领域类型由 Codix 自己定义，翻译工作在 `host` 里做。
- `app` 不依赖 Agix。它只通过 `Backend` 和后端说话。

## 3. 端口 `port`

### 3.1 trait

```rust
#[async_trait]
pub trait Backend: Send + Sync + 'static {
    /// 发一条命令，等它的结果。实现方不得在这里阻塞事件流。
    async fn call(&self, cmd: Command) -> Result<Reply, BackendError>;
    /// 领域事件流。每个 Backend 只有一个订阅者，也就是 app。
    fn events(&self) -> mpsc::Receiver<Event>;
}
```

app 把每个 `call` 都 spawn 出去，结果以 `Msg::Reply { req, result }` 投回事件循环。所以没有任何一个后端调用会卡住界面（N-03）。

### 3.2 命令（`Command`）

按对象分组，名字是领域语言，不是 Agix 的方法名。

| 组 | 命令 |
|---|---|
| 连接 | `Hello`（取能力位：流程和作业是否可用） |
| 会话 | `ListSessions{filter}`、`CreateSession{project, scene, model}`、`OpenSession{id}`（必要时恢复）、`Watch{id}` / `Unwatch{id}`（订阅或退订它的细节事件）、`History{id, before}`、`Rename`、`Close` |
| 对话 | `Submit{session, text, mode: Turn \| Steer \| Queue}`、`Interrupt{session}`、`Answer{session, prompt, answer}`、`SetModel`、`Commands{session}`、`SideQuestion{session, text}` |
| goal | `SetGoal`、`UpdateGoal{action: Pause \| Resume \| Edit \| Clear}` |
| 任务 | `StopTask`、`MessageTask`、`TaskOutput{task, from_byte}` |
| 流程 | `ListFlows`、`GetFlow{name}`、`ListRuns{flow}`、`GetRun{id}`、`StartRun{flow, subject, inputs}`、`RunOp{run, op}`（`op` 为 Confirm / Release / Force{reason} / Rework{to} / Retry / Pause / Resume / Cancel）、`OpenAuthoring{flow?}` |
| 作业 | `ListJobs`、`SaveJob`、`DeleteJob`、`SetJobEnabled`、`RunJobNow`、`JobHistory{job}` |

### 3.3 事件（`Event`）

事件已经归一化成领域语言，model 不需要知道 Agix 的帧长什么样。

| 组 | 事件 |
|---|---|
| 连接 | `Connection(Connected \| Lost \| Reconnecting \| Resynced)` |
| 会话列表 | `SessionUpserted(SessionSummary)`、`SessionRemoved(id)`。所有会话都推，只推摘要：状态、在等人的数量、token |
| 一轮（仅限 Watch 中的会话） | `TurnStarted`、`StepRestarted{step}`（丢掉这一步已画的文字）、`TextDelta`、`ThinkingDelta`、`ToolStarted{id, name}`、`ToolInput{id, input}`、`ToolProgress`、`ToolFinished{id, content, is_error, file_changes}`、`TurnEnded{stop, usage}`、`TurnFailed{error}` |
| 插话 | `SteerDelivered`、`SteerReturned{messages}` |
| 提问 | `PromptPending(Prompt)`、`PromptResolved{id, resolution}` |
| 会话状态 | `GoalChanged`、`TodosChanged`、`Compacted`、`CommandsChanged`、`Notice{level, text}` |
| 任务 | `TaskUpserted(Task)`（子 Agent 的 `session` 字段指向它的侧链会话）、`TaskOutput{task, bytes, from, next}` |
| 流程 | `RunUpserted(RunSummary)`、`RunDetail(RunView)`（选中的运行才推） |
| 作业 | `JobUpserted(Job)`、`JobFired{job, entry}` |

**追赶由后端负责**：先订阅，再读历史，把订阅之后缓冲的事件接在后面。model 收到的永远是"一个快照（`Reply`）+ 之后的增量"，不处理接缝。重连之后先推 `Connection(Resynced)`，再推新的快照。

### 3.4 数据类型

- 所有 id 都是不透明的字符串新类型（`SessionId`、`TaskId`、`PromptId`、`RunId`、`JobId`）。
- 时间统一用毫秒时间戳（`u64`），用量统一用 `Usage{input, output, cache_read, cache_write}`。
- 任务的 `kind` 是枚举（`Agent`、`Teammate`、`Shell`、`Monitor`、`Code`、`Mcp`、`Cron`），另外带一个 `Unknown(String)`。引擎加了新类型，界面照样能画，显示原样的名字。
- 流程的状态枚举与 Harnix 一致：运行 `Active / Finished / Cancelled`，加上派生出来的"等待 / 暂停中 / 已暂停"；站 `Pending / Held / Working / Unmet / Passed / Skipped / Stale`。阶段 3 由 `host` 从 harnix-flow 的类型翻译过来，`port` 不直接引用它们。

## 4. view-model `model`

### 4.1 状态

```
State
├── domain        后端事实，只由 Event / Reply 写入
│   ├── sessions      会话摘要；被 Watch 的会话还有 Transcript、AskQueue、Tasks、Goal、Todos
│   ├── flows / runs  流程定义、运行摘要、选中运行的详情
│   ├── jobs          作业与触发历史
│   └── conn          连接状态、能力位
└── ui            界面状态，只由按键和 Tick 写入
    ├── layout        并排 / 单栏、比例、右栏是否收起
    ├── focus         Composer / Transcript / Right(Upper | Lower) / Dialog
    ├── context       对话区当前的上下文栈：会话 › 子 Agent › …（面包屑）
    ├── tabs          当前标签；每个标签的展开、选中、滚动
    ├── composer      草稿、光标、历史、补全
    └── transcript    每个上下文的滚动位置、选中块、折叠集合
```

**一份 `FrameState` 只有一个写者，就是 `view`。** 旧实现里 reducer 和 app 各填半份 `FrameState`，这一点不能再犯。

### 4.2 `update` / `view`

- `update(&mut State, Msg) -> Vec<Effect>`：`Msg` 是 `Action(动作)`、`Text(char)`、`Paste(String)`、`Event`、`Reply`、`Tick`、`Resize`；`Effect` 是 `Call(Command)`、`Quit`、`Bell`。
- `view(&State, Size) -> FrameState`：先调 `tui::layout::compute(size, &ui.layout)` 拿到各区域的矩形，再按每个区域的宽高投影。渲染端用同一个函数算矩形，两边不会对不上。

### 4.3 转录：一条 entry 就是屏幕上的一行

- 转录由**块**组成：用户输入、回答、思考、工具调用、通知、错误。每个块有稳定的 id。
- 每个块缓存它在某个宽度下排好的行，排版由 model 完成：按显示宽度折行，按 `\n` 分行，CJK 和 emoji 按宽度计算。`FrameState` 里只有视口内已经排好的行，**渲染端不折行**。
- 流式增量只把它所在的块标脏，下一帧只重排脏块，与转录长度无关（N-02）。宽度一变，所有块的缓存都失效，但只有视口附近的块会被立刻重排。
- 行上带 `block_id` 和 `first_of_block`。**语义只有一种**：`first_of_block == true` 表示这是块的第一行。凡是"按块还原"的逻辑（输入历史、置顶的提问行），都只能用 `model` 里同一个辅助函数，不允许各自去解释这个字段。

### 4.4 待人处理（L-04、L-05）

`model` 维护一个派生的"等你处理"列表，在每次 `update` 之后重算，来源包括：

- 各会话的 `waiting` 数量
- 搁置或待确认的站
- 受阻的 goal
- 失败的作业

标签上的角标和"跳到下一处"都读这个列表。

## 5. 事件循环 `app`

```
loop {
    select! {
        key   = terminal.next()      => msgs.push(resolve(key, state.key_context())),
        ev    = backend_events.recv() => msgs.push(Msg::Event(ev)),
        reply = replies.recv()        => msgs.push(Msg::Reply(reply)),
        _     = ticker, if animating  => msgs.push(Msg::Tick),
    }
    // 把这一刻已经到达的消息全部处理完，再画一帧
    while let Ok(m) = drain() { effects += update(&mut state, m) }
    run(effects);                      // Call → spawn；Quit → 退出
    if dirty && frame_budget_ok { terminal.draw(view(&state, size)) }
}
```

- **合帧**：一次循环把这一刻已经到达的消息全部消化完，最多画一帧，帧间隔下限约 16ms。事件风暴时，每个事件只付出 `update` 的代价。
- **Tick 只在有动画时开启**，比如 spinner 和状态行计时。没有动画就不唤醒。
- 终端的进入和退出（raw mode、备用屏、bracketed paste、键盘增强协议）集中在一处管理。panic 时先恢复终端，再打印错误。

## 6. 快捷键 `keys`

- 绑定是 `(上下文, 按键或和弦) → 动作`。上下文有 `global`、`composer`、`transcript`、`sessions`、`flows`、`jobs`、`dialog`、`picker`。
- 解析顺序：先查当前上下文，再查 `global`。同一个上下文里同一个键只能绑一个动作；冲突在加载时报错，不能静默覆盖。
- 普通字符按键在 `composer` 上下文里一律当作输入。只有在列表和对话框的上下文里，字母才是快捷键。旧实现里"同一个 `y` 在不同区域有时是输入、有时是快捷键"的猜测逻辑不再需要。
- 和弦有超时（1.5 秒），超时后把已按下的键按普通键处理。
- 用户覆盖文件：`<配置目录>/keybindings.json`（D-02，P1）。

完整的键位表见 `interaction.md`。

## 7. mock 后端 `mock`

详见 `testing.md`。要点如下：

- `ScriptedBackend` 实现 `Backend`。状态由**种子数据**初始化（会话、任务、流程、运行、作业），之后由**场景**驱动。
- 场景是一条带时间点的事件时间线，再加一组"命令 → 反应"的规则（比如对 `Submit` 回放一段带工具调用的回答，对 `RunOp::Confirm` 推进一站）。
- 时钟可以注入：交互运行时用真实时钟（并支持 `--speed` 加速），测试时用 tokio 的暂停时钟，结果完全确定。
- 不依赖 Agix、Harnix 或网络。

## 8. 阶段 3：接缝（只写到接口这一层）

### 8.1 两条独立的传输

| 段 | 可选项 | 决定什么 |
|---|---|---|
| TUI ↔ 宿主 | 同进程（duplex）/ 远端 `codix serve`（unix socket 或 WebSocket + token） | 断开终端后，流程和作业还在不在跑 |
| 宿主 ↔ Agix | `agix::embed` 同进程 / 远端 `agixd` | 引擎在哪台机器上跑 |

两段可以自由组合。远程使用的主形态是"`codix serve` 在服务器上，引擎嵌在它里面"。

### 8.2 Agix 接入

- 只走 JSON-RPC 协议（`agix/docs/daemon_rpc_protocol.md`），不使用 `agix::pool`。
- 宿主侧的进程职责（agix-sdk.md §7）：
  - tracing 的过滤器用 `agix::process::log_filter`，否则 rmcp 会把 OAuth 令牌写进日志
  - 安装 panic 钩子
  - 收到信号时调用 `shutdown`
  - 用 `groups_file` 回收孤儿进程组
- 建会话时声明 `prompt_types: ["permission", "question"]`。
- 帧到事件的翻译：

| Agix 帧 / 方法 | 领域事件 / 命令 |
|---|---|
| `turn_start` / `step_start`（同一个 step 第二次出现） | `TurnStarted` / `StepRestarted` |
| `text_delta` / `thinking_delta` | `TextDelta` / `ThinkingDelta` |
| `tool_use_start` / `tool_use` / `tool_progress` / `tool_result` | `ToolStarted` / `ToolInput` / `ToolProgress` / `ToolFinished` |
| `tool_use` 的工具名为 `TodoWrite` | 另外推一条 `TodosChanged` |
| `prompt` / `prompt_resolved` | `PromptPending` / `PromptResolved` |
| `turn_complete` / `error` 帧或 `run_turn` 的错误 | `TurnEnded` / `TurnFailed` |
| `user_message_delivered` / `user_messages_returned` | `SteerDelivered` / `SteerReturned` |
| `task_started` / `task_updated` / `task_finished` / `task_progress` | `TaskUpserted` / `TaskOutput` |
| `subagent_progress` | 子 Agent 侧链会话的细节事件（Watch 时） |
| `goal_changed` / `compact` / `skills_changed` / `notification` | `GoalChanged` / `Compacted` / `CommandsChanged` / `Notice` |
| `Submit{mode}` | `session.run_turn{on_busy: reject \| steer \| queue}` |
| `Interrupt` / `Answer` / `SetModel` | `session.interrupt` / `session.respondToPrompt` / `session.setModel` |
| `SideQuestion` | `session.fork` 之后在分叉会话上 `run_turn` |
| `StopTask` / `MessageTask` / `TaskOutput` | `session.stopTask` / `session.messageTask` / `session.taskOutput` |

### 8.3 流程

- 流程代码从 Harnix 拷贝到 `3rds/harnix-flow/`（内核 + runner）。本地不改，同步规则见 `3rds/INDEX.md`。
- `host` 用 Agix 客户端实现 `FlowEngine`，内容包括：开会话、跑一轮、起子 Agent、回答询问、登记 `flow_submit` / `flow_state` / `flow_propose` 宿主工具、订阅帧。
- 流程会话里的权限询问由宿主按这一站的策略作答，策略写的是 `ask` 时才转给人，在界面上表现为一条 `PromptPending`。
- 编写会话的实现方式：`session.setPrompt` 加上编写用的宿主工具。
- 前提：Harnix#66 拆出 `crates/flow`，并在其中加入 runner 和 `FlowEngine`。

### 8.4 作业

- 宿主自带调度器，作业定义和触发历史都由宿主持久化。
- 简单作业：在指定的项目和场景里建会话（或复用会话），然后提交提示词。
- 流程作业：调用 `StartRun`。
- 宿主不在时错过的触发，按作业的 `catch_up` 配置决定补跑一次还是跳过。

### 8.5 `codix serve`

- 同一个二进制的子命令，不带 TUI。
- 运行在 systemd / launchd 下，处理 SIGTERM，写崩溃记录。
- 认证：unix socket 用文件权限 0600，配合 ssh 转发使用；WebSocket 必须带 token。

## 9. 旧实现踩过的坑（必须避开）

| 坑 | 本设计怎么避开 |
|---|---|
| 一个"块首行"标记被产出方和消费方理解成相反的意思，测试辅助按错误的理解造数据，所以测试全是绿的 | §4.3：只有一种语义，只有一个辅助函数；测试必须用 `view` 的真实输出，不允许手工拼 `FrameState` |
| 渲染端开了自动折行，破坏了"一条 entry 一行"，视口按条数取内容，贴底时最新的行被挤出屏幕 | §4.3：由 model 排版，渲染端不折行 |
| ratatui 的 `Line` 遇到 `\n` 会直接吞掉，不换行也不留空格 | 排版时先按 `\n` 拆行，单元测试覆盖 |
| 每个 token 都重建整份转录，再把整帧 clone 一次 | §4.3 块级缓存，§5 合帧 |
| 在 `select!` 的分支里直接 await 读盘，读的期间界面卡住 | §3.1：所有调用都 spawn 出去，结果以 Msg 投回 |
| `FrameState` 由两方各填一半，同一个派生量两处各算一次 | §4.1：`view` 是唯一的写者 |
| 键位层没有上下文，只好给每个模态起别名，裸字符时而是输入、时而是快捷键 | §6：键位带上下文 |
| 契约里有一批字段从来没人产出 | 阶段 2 每个 `FrameState` 字段都必须有一个场景和一张快照覆盖到 |

## 10. 待定

| 问题 | 倾向 |
|---|---|
| 数据目录：Agix 的数据和 Codix 自己的流程、作业放在哪 | Agix 用 `~/.agix`，与 `agixd` 共用；Codix 用 `~/.codix`（流程包、运行、作业、快捷键）。阶段 3 再定 |
| 宽屏下左右两栏的默认比例 | 60 : 40，可调，并记住 |
| `codix serve` 的 WebSocket 是否需要 TLS | 不自带 TLS，跨公网时前面加反向代理 |

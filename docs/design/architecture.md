# Codix 架构

需求见 `docs/reqs/codix-requirements.md`，交互见 `interaction.md`，测试见 `testing.md`。

## 1. 总览

Codix 采用 **"常驻实例 + 可接入的 TUI"** 两进程结构，和 tmux 的 server / client 类似，区别在于：**一个实例同一时刻只接受一个 TUI。**

```
本机（远程时就是 ssh 登录上去的那台机器）

┌──── codix（TUI 进程；接入同一实例的只能有一个）──────────────────┐
│  终端（按键 / 鼠标 / 尺寸）──► keys ──► app 事件循环              │
│                                         │                        │
│               model::update / model::view ──► tui::render        │
│                                         │                        │
│                    port::Backend  ◄─────┘   port::Instances      │
│                         │                  （列出 / 新建 / 接入） │
└─────────────────────────┼────────────────────────────────────────┘
                          │ unix socket（0600，校验对端 uid）
                          │ 消息 = port 的 Command / Reply / Event，编码为 NDJSON
┌─────────────────────────▼──── codix host（实例进程，常驻）───────┐
│  会话代理 ── Agix 客户端 ──► agix::embed（同进程，data_root = ~/.codix）│
│  流程（3rds/harnix-flow 内核 + runner，FlowEngine 由 Agix 实现）   │
│  作业调度                                                         │
│  单一接入：占用、强制接入、空闲脱离、心跳；状态落盘在实例目录     │
└──────────────────────────────────────────────────────────────────┘
```

设计要点：

1. **TUI 只认 `port`。** 阶段 2 由 mock 实现 `port`，阶段 3 换成真实实现，界面一行都不用改。
2. **`port` 的消息就是实例协议。** `Command`、`Reply`、`Event` 都可以序列化，放到 unix socket 上就是 TUI 与实例之间的协议，不再另外设计。
3. **实例一律跑在独立进程里。** 新建实例就是拉起一个脱离终端的 `codix host` 进程，再让 TUI 接入它。退出 TUI 等于脱离，流程和作业不会因为终端关掉而停下。
4. **只有本机 socket。** 不监听任何网络端口。远程使用就是 ssh 登录后在那台机器上运行 `codix`，安全交给 ssh 和操作系统。

## 2. crate 划分

| crate | 阶段 | 职责 | 依赖 |
|---|---|---|---|
| `port` | 2 | 端口定义：`Instances` 与 `Backend` 两个 trait；`Command`、`Reply`、`Event`；领域数据类型（实例信息、会话摘要与归属、任务、goal、todo、流程、运行、站、作业……） | serde, async-trait, tokio（mpsc） |
| `tui` | 2 | 纯渲染：把 `FrameState` 画到屏幕上；`layout::compute` 是纯函数，算出各区域的矩形，同时给出鼠标命中测试用的区域表 | ratatui, crossterm, unicode-width |
| `keys` | 2 | 快捷键：解析、带上下文的绑定表、和弦状态机、用户覆盖与冲突校验 | crossterm, serde |
| `model` | 2 | view-model：`State`（领域状态 + 界面状态）、`update`、`view`；转录块的排版缓存；鼠标事件到动作的翻译 | port, tui（只用 `FrameState` 类型与 `layout`）, keys（只用动作与上下文类型） |
| `mock` | 2 | `MockInstances` 与 `ScriptedBackend`：用种子数据和场景脚本驱动 | port |
| `app` | 2 | 二进制 `codix`：命令行解析、终端生命周期（尺寸协商、鼠标捕获）、事件循环、实例选择列表 | 以上全部 |
| `wire` | 3 | Agix 协议的 serde 类型（协议 §4、§7）。遇到不认识的 `kind` 一律跳过 | serde |
| `host` | 3 | 实例进程：Agix 客户端、把 Agix 帧翻译成领域事件、流程（`FlowEngine` 的实现）、作业调度、socket 服务与单一接入（占用、强制接入、心跳）、实例锁与发现文件 | port, wire, agix, 3rds/harnix-flow |
| `client` | 3 | `SocketInstances` 与 `SocketBackend`：扫描实例、拉起实例、通过 socket 接入 | port |

**依赖边界**（阶段 2 起用测试守住）：

- `tui` 不依赖 `port`、`model`，也不依赖 Agix。
- `model` 不做 IO、不起任务，`update` 和 `view` 都是同步的纯函数。
- `port` 不依赖任何 Agix 或 Harnix 的类型，翻译工作在 `host` 里做。
- `app` 不依赖 Agix，只通过 `port` 与后端打交道。阶段 3 的 `codix host` 子命令由 `app` 转交给 `host` crate 处理。

## 3. 端口 `port`

### 3.1 两个 trait

```rust
#[async_trait]
pub trait Instances: Send + Sync + 'static {
    async fn list(&self, all: bool) -> Result<Vec<InstanceInfo>, PortError>;
    async fn create(&self, opts: NewInstance) -> Result<Arc<dyn Backend>, PortError>;
    /// 实例已被别的 TUI 接入时返回 `PortError::Occupied { holder, idle_ms }`；
    /// `force` 为 true 则踢掉占用者再接入。
    async fn attach(&self, name: &str, opts: Attach) -> Result<Arc<dyn Backend>, PortError>;
    async fn start(&self, name: &str) -> Result<Arc<dyn Backend>, PortError>;
    async fn stop(&self, name: &str) -> Result<(), PortError>;
}

#[async_trait]
pub trait Backend: Send + Sync + 'static {
    async fn call(&self, cmd: Command) -> Result<Reply, PortError>;
    fn events(&self) -> mpsc::Receiver<Event>;   // 每个 Backend 只有一个订阅者，就是 app
}
```

- `InstanceInfo` 包含：名字、PID、主机名、项目、启动时间、版本、会话数、在等人的数量、状态（运行中 / 已停止 / 残留），以及 `holder: Option<ClientInfo>`，即当前接入者。
- `ClientInfo` 包含：用户、PID、终端（tty）、ssh 来源（取自 `SSH_CONNECTION`）、接入时刻、最后一次输入的时刻。
- `Attach` 包含：`force`，以及 `idle_timeout`（可选，覆盖配置里的值）。
- app 把每次 `call` 都 spawn 出去，结果通过 `Msg::Reply` 投回事件循环，没有哪个调用会卡住界面（N-03）。

### 3.2 命令（`Command`）

| 组 | 命令 |
|---|---|
| 连接 | `Hello{client, force}`：返回实例信息和能力位；`Heartbeat{last_input_ms}`；`Detach`；`StopInstance` |
| 会话 | `ListSessions{filter}`、`CreateSession{project, scene, model}`、`OpenSession{id}`（需要时先恢复）、`Watch{id}` / `Unwatch{id}`、`History{id, before}`、`Rename`、`Close` |
| 对话 | `Submit{session, text, mode: Turn \| Steer \| Queue}`、`Interrupt`、`Answer{session, prompt, answer}`、`SetModel`、`Commands{session}`、`SideQuestion{session, text}` |
| goal | `SetGoal`、`UpdateGoal{action: Pause \| Resume \| Edit \| Clear}` |
| 任务 | `StopTask`、`MessageTask`、`TaskOutput{task, from_byte}` |
| 流程 | `ListFlows`、`GetFlow{name}`、`ListRuns{flow}`、`GetRun{id}`、`StartRun{flow, subject, inputs}`、`RunOp{run, op}`、`OpenAuthoring{flow?}` |
| 作业 | `ListJobs`、`SaveJob`、`DeleteJob`、`SetJobEnabled`、`RunJobNow`、`JobHistory{job}` |

### 3.3 事件（`Event`）

| 组 | 事件 |
|---|---|
| 连接 | `Connection(Connected \| Lost \| Reconnecting \| Resynced)`、`Kicked{by: ClientInfo, at}`（被强制接入的一方踢掉）、`IdleDetached{after_ms}` |
| 会话列表 | `SessionUpserted(SessionSummary)`、`SessionRemoved(id)` |
| 一轮（仅限被 Watch 的会话） | `TurnStarted`、`StepRestarted{step}`、`TextDelta`、`ThinkingDelta`、`ToolStarted`、`ToolInput`、`ToolProgress`、`ToolFinished`、`TurnEnded{stop, usage}`、`TurnFailed` |
| 插话 | `SteerDelivered`、`SteerReturned{messages}` |
| 提问 | `PromptPending(Prompt)`、`PromptResolved{id, resolution}` |
| 会话状态 | `GoalChanged`、`TodosChanged`、`Compacted`、`CommandsChanged`、`Notice{level, text}` |
| 任务 | `TaskUpserted(Task)`、`TaskOutput{task, bytes, from, next}` |
| 流程 | `RunUpserted(RunSummary)`、`RunDetail(RunView)` |
| 作业 | `JobUpserted(Job)`、`JobFired{job, entry}` |

**追赶由后端负责。** model 看到的永远是"先一个快照（`Reply`），再接着增量"。重连成功后，后端先推 `Connection(Resynced)`，再推一个新的快照。

### 3.4 数据类型

- id 一律用不透明的字符串新类型。时间一律是毫秒时间戳。用量统一为 `Usage{input, output, cache_read, cache_write}`。
- **会话归属** `Provenance`：`instance`、`host`、`project`、`scene`、`model`、`origin`。其中 `origin` 取值为 `User | Job{job} | FlowStation{run, station} | SubAgent{parent, task} | Fork{from}`（C-12）。
- 任务的 `kind`、站和运行的状态都是枚举，并带一个 `Unknown(String)` 兜底。站和运行的状态与 Harnix 保持一致。

## 4. view-model `model`

### 4.1 状态

```
State
├── domain        后端事实，只由 Event / Reply 写入
│   ├── instance      当前接入的实例信息、能力位
│   ├── sessions      会话摘要与归属；被 Watch 的会话另有 Transcript / AskQueue / Tasks / Goal / Todos
│   ├── flows / runs  流程定义、运行摘要、选中运行的详情
│   ├── jobs          作业与触发历史
│   └── conn          连接状态
└── ui            界面状态，只由输入（按键、鼠标）和 Tick 写入
    ├── screen        实例选择 / 工作台 / 已脱离（被踢、空闲超时）
    ├── layout        并排或单栏、两栏比例、右栏是否收起、当前显示哪一栏（单栏时）
    ├── focus         Composer / Transcript / Side(Upper | Lower) / Dialog
    ├── context       对话区的上下文栈（面包屑）
    ├── tabs          当前标签；每个标签各自的展开、选中、滚动状态
    ├── composer      草稿、光标、历史、补全
    ├── transcript    每个上下文各自的滚动位置、选中块、折叠集合、拖选范围
    ├── mouse         是否捕获鼠标、正在拖动的对象（分隔线 / 选区）
    └── idle          最后一次输入的时刻；空闲超时的时长
```

**`FrameState` 只有 `view` 一个写者。**

### 4.2 `update` / `view`

- `update(&mut State, Msg) -> Vec<Effect>`
  - `Msg`：`Action`、`Text`、`Paste`、`Mouse{kind, col, row}`、`Event`、`Reply`、`Tick`、`Resize`
  - `Effect`：`Call(Command)`、`Instances(op)`、`Quit{detach}`、`Clipboard(String)`、`SetMouseCapture(bool)`、`Bell`
- `view(&State, Size) -> FrameState`：先调用 `tui::layout::compute`，拿到各区域的矩形，再分别投影。
- **鼠标命中测试**：`layout::compute` 同时返回区域表（矩形 → 区域路径）。model 记住上一帧每一行对应什么（列表项、块头、选项、分隔线）。鼠标事件按"区域 + 行"找到目标，再翻译成和键盘相同的动作。鼠标没有任何独立的逻辑分支，所以键盘能做的鼠标都能做，测试也只需要覆盖一份动作逻辑。

### 4.3 转录：一条 entry 就是屏幕上的一行

- 转录由**块**组成，每个块有稳定的 id。每个块缓存它在当前宽度下排好的行，排版在 model 里完成：按显示宽度折行，按 `\n` 分行，CJK 和 emoji 按实际宽度计算。**渲染端不做折行。**
- 流式增量只把它所在的那个块标脏，与转录长度无关（N-02）。
- 行上带 `block_id` 和 `first_of_block`，后者只有一种语义：这一行是块的第一行。凡是"按块还原"的逻辑，都只能调用 `model` 里的同一个辅助函数。
- 拖选复制（M-07）：按"块 + 行 + 列"记录选区，复制时从块的原文中取，不从排版后的行拼。这样折行处不会混进多余的换行。

### 4.4 选中即切换与卡片

- 每次 `update` 之后（`follow::after_update`）：右栏当前看得见的东西缺数据就去取（运行列表、
  流程定义、运行详情、作业历史、被选中会话的快照）；选中变了且目标带会话，就让对话区显示它。
- 流程与作业的编辑会话由 `OpenAuthoring` / `OpenJobSession` 取得（同一个对象总是同一个会话），
  会话里钉一个卡片块（`BlockKind::Card`），数据（`FlowDetail`、作业与历史）变了就刷新它。
- 新建、修改、清理流程与作业走对话；阶段 3 由引擎侧的流程、作业工具执行，界面不另开表单。

### 4.5 待人处理

`model` 维护一个派生列表："等你处理"的事项。来源有：各会话中在等人的项、搁置或待确认的站、受阻的 goal、失败的作业。标签角标和"跳到下一处"都读这一份列表。

## 5. 事件循环与终端 `app`

```
loop {
    select! {
        input = terminal.next()      => msgs.push(translate(input, &state)),  // 按键 / 鼠标 / 尺寸 / 粘贴
        ev    = backend_events.recv() => msgs.push(Msg::Event(ev)),
        reply = replies.recv()        => msgs.push(Msg::Reply(reply)),
        _     = ticker, if animating  => msgs.push(Msg::Tick),
    }
    while let Ok(m) = drain() { effects += update(&mut state, m) }
    run(effects);
    if dirty && frame_budget_ok { terminal.draw(view(&state, size)) }
}
```

- **合帧**：每轮循环先把这一刻已经到达的消息全部处理完，最多画一帧，帧间隔下限约 16ms。
- **Tick 只在有动画时开启**（比如 spinner、计时器）。

**终端生命周期**集中在一处，按下面的顺序进入，退出时按相反顺序恢复；panic 时也要先恢复终端：

| 步骤 | 内容 |
|---|---|
| 1 | 尺寸协商（L-06）：没有在 tmux 或 screen 里、且配置没有关掉时，先记下当前尺寸，再发送 `CSI 8 ; 行 ; 列 t` 请求缺省尺寸，最多等 300ms 的 Resize 事件。等不到就按实际尺寸继续。退出时如果改过尺寸，再请求恢复到原来的尺寸 |
| 2 | raw mode、备用屏 |
| 3 | 鼠标捕获（按下与松开、拖动、滚轮）。可以在运行中切换（M-08） |
| 4 | bracketed paste |
| 5 | 键盘增强协议（kitty），终端支持时才开启，用来区分 Shift+Enter 等组合键 |

**剪贴板**：写剪贴板用 OSC 52，ssh 上也能用。tmux 里需要打开 `set-clipboard`，这一点写进手册。

**命令行**：

| 命令 | 行为 |
|---|---|
| `codix` | 本机有在跑的实例时，先显示实例选择列表；没有就新建一个并接入 |
| `codix new [--name] [--project]` | 新建实例并接入 |
| `codix list [--all]` | 用纯文本列出实例，不进入 TUI |
| `codix attach <名字> [--force] [--idle-timeout <时长>]` | 接入指定实例；`--force` 踢掉当前接入者 |
| `codix start <名字>` / `stop <名字>` | 启动 / 停止指定实例 |
| `codix --mock <场景>` | 阶段 2：使用 mock 的实例和后端 |
| `codix host --name <名字>` | 内部使用：实例进程的入口，由 `new` / `start` 拉起，用户不直接调用 |

## 6. 快捷键 `keys`

- 绑定的形式是 `(上下文, 按键或和弦) → 动作`。上下文有：`global`、`composer`、`transcript`、`sessions`、`flows`、`jobs`、`dialog`、`picker`、`instances`。
- 查找顺序：先查当前上下文，再查 `global`。同一个上下文里，同一个键只能绑定一个动作；有冲突时在加载阶段就报错。
- 在 `composer` 上下文里，普通字符一律作为输入；字母快捷键只在列表和对话框里生效。
- 和弦超时 1.5 秒。用户的覆盖文件放在 `~/.codix/keybindings.json`。
- **底栏的保底提示**：不论处在哪个上下文，底栏的最后两项永远是"切换栏位"和"帮助"（L-07）。这两项由 `keys` 从绑定表里取出实际的键位，用户改了键位，底栏也跟着变。

## 7. mock `mock`

见 `testing.md`。`MockInstances` 用种子数据提供几个"在跑"的实例和几个"已停止"的实例，`create` 和 `attach` 返回一个 `ScriptedBackend`。阶段 2 不会真的起任何进程。

## 8. 阶段 3：实例（这里只写到接口层面）

### 8.1 数据目录

```
~/.codix/                         CODIX_HOME 指定时以它为准；同时也是 Agix 的 data_root
├── settings.json                 ┐
├── projects/  sessions/  memory/ │ Agix 的全局数据，布局归 Agix（ARCHITECTURE.md §8）。
├── scenes/  daemon/  …           ┘ 模型、provider、权限等配置都沿用这里的 settings.json
├── codix.yaml                    Codix 自己的配置（YAML）：界面（缺省尺寸、比例、主题配色、语法高亮主题、
│                                 工具结果显示几行）、实例（空闲超时）；不存在时写一份带注释的缺省配置
├── keybindings.json              快捷键覆盖
├── flows/                        流程包（流程、skills、规则、策略、门禁、钩子），所有实例共用
└── instances/<名字>/
    ├── instance.json             发现文件：pid、socket、项目、启动时间、版本、当前接入者
    ├── instance.lock             flock，防止同名实例跑两个
    ├── host.sock                 0600，目录 0700
    ├── groups.json               Agix 的 groups_file，用于回收孤儿进程组
    ├── runs/                     本实例的流程运行（harnix-flow 的 FileStore）
    ├── jobs/                     本实例的作业定义与触发历史
    └── host.log
```

- Codix 自己的文件和 Agix 的放在同一个目录，用 Codix 自己的名字（`codix.yaml`、`keybindings.json`、`flows/`、`instances/`），都不和 Agix 的顶层名字冲突。**不往 Agix 的 `settings.json` 里写 Codix 的配置**：Agix 对不认识的键会告警，而且两边的配置应当各管各的。
- **流程包所有实例共用；运行和作业属于各自的实例。** 这样两个实例不会重复触发同一个作业。`codix start <名字>` 重启实例时，会把它的运行和作业接着跑起来。
- 项目层目录：Agix#86 落地之前沿用 `<项目>/.agix/`，落地之后通过 `EmbedConfig.project_dir` 改成 `.codix/`。Codix 自己目前不在项目里放任何东西。

### 8.2 实例进程

- `codix new` 的流程：拿到 `instance.lock` → 用 `setsid` 拉起脱离终端的 `codix host` → 等 `host.sock` 可以连接 → 接入。
- 引擎嵌在实例进程里：`agix::embed`，`EmbedConfig::new(~/.codix)`，**不传 `instance`**。原因是 Agix 的实例锁按"数据根 + 场景"计算，同一个场景只能锁一份，所以实例锁由 Codix 自己管；`groups_file` 指向实例目录下的 `groups.json`。
- 实例进程要做的进程级工作（agix-sdk.md §7）：
  - 日志过滤用 `agix::process::log_filter`，否则 rmcp 会把 OAuth 令牌写进日志
  - 安装 panic 钩子
  - 收到 SIGTERM 时依次关闭会话、流程、作业，再删掉 socket 和发现文件
- **单一接入**：
  - 实例同时只服务一个 TUI 连接。另一个 `Hello` 到来时，如果不带 `force`，就回 `Occupied{holder, idle_ms}`；带 `force` 时，先给当前连接推一条 `Kicked{by, at}`，再关闭它，然后接受新连接。
  - 被踢、空闲脱离、连接断开，都不影响实例里正在进行的工作。
  - **心跳**：TUI 每 10 秒发一次 `Heartbeat`，带上最后一次输入的时刻。实例连续 3 次没收到心跳，就断开这个连接，处理 TUI 僵死、ssh 半开这类情况。
  - **空闲脱离**：由 TUI 自己判断。超过 `idle_timeout` 没有任何输入，就发 `Detach` 并进入"已脱离"界面，终端不退出，按任意键重新接入。实例那边再以心跳里的 `last_input_ms` 兜底：如果客户端没执行空闲脱离（比如是旧版本），实例按配置把它断开。
- 残留检测：`list` 时遇到 pid 已经不在、或者 socket 连不上的实例，标记为"残留"，并清理掉 socket 和发现文件；运行和作业的状态保留，可以用 `start` 再拉起来。
- 对端校验：accept 之后用 `getpeereid` / `SO_PEERCRED` 检查对端 uid，必须与实例进程的 uid 一致。

### 8.3 Agix 接入

- 只走 JSON-RPC 协议：实例进程内部用 `Embedded::serve_stream` 加 duplex 和引擎通信，不使用 `agix::pool`。
- 建会话时声明 `prompt_types: ["permission", "question"]`。

帧与事件的对应关系：

| Agix 帧 / 方法 | 领域事件 / 命令 |
|---|---|
| `turn_start` / 同一个 step 再次出现的 `step_start` | `TurnStarted` / `StepRestarted` |
| `text_delta` / `thinking_delta` | `TextDelta` / `ThinkingDelta` |
| `tool_use_start` / `tool_use` / `tool_progress` / `tool_result` | `ToolStarted` / `ToolInput` / `ToolProgress` / `ToolFinished` |
| 工具名为 `TodoWrite` 的 `tool_use` | 额外推一个 `TodosChanged` |
| `prompt` / `prompt_resolved` | `PromptPending` / `PromptResolved` |
| `turn_complete` / `error` 帧，或 `run_turn` 返回的错误 | `TurnEnded` / `TurnFailed` |
| `user_message_delivered` / `user_messages_returned` | `SteerDelivered` / `SteerReturned` |
| `task_started` / `task_updated` / `task_finished` / `task_progress` | `TaskUpserted` / `TaskOutput` |
| `goal_changed` / `compact` / `skills_changed` / `notification` | `GoalChanged` / `Compacted` / `CommandsChanged` / `Notice` |
| `Submit{mode}` | `session.run_turn{on_busy}` |
| `Interrupt` / `Answer` / `SetModel` | `session.interrupt` / `session.respondToPrompt` / `session.setModel` |
| `SideQuestion` | 先 `session.fork`，再在分叉出的会话上 `run_turn` |
| `StopTask` / `MessageTask` / `TaskOutput` | `session.stopTask` / `session.messageTask` / `session.taskOutput` |

会话归属：`origin` 由实例自己记录（是哪个作业、哪一站、哪次分叉建的会话）；子 Agent 的上级来自 `session.list{parent_session_id}`。

### 8.4 流程与作业

- 流程代码拷贝到 `3rds/harnix-flow/`（内核 + runner）。实例用 Agix 客户端实现 `FlowEngine`。流程会话里的权限询问由实例按站的策略作答，策略为 `ask` 的才转给人。前提是 Harnix#66 已经完成。
- 作业由实例自带的调度器执行。简单作业：建会话或复用会话，再提交提示词。流程作业：调用 `StartRun`。实例没在运行期间错过的触发，按作业的 `catch_up` 设置处理。

## 9. 旧实现踩过的坑

| 坑 | 本设计如何避开 |
|---|---|
| "块首行"标记在产出方和消费方的理解正好相反，而测试辅助函数是按错误的理解造数据的，所以测试全绿 | §4.3：只有一种语义，只有一个辅助函数；测试只能用 `view` 的真实输出 |
| 渲染端自动折行，破坏了"一条 entry 一行"的约定 | §4.3：由 model 排版，渲染端不折行 |
| ratatui 的 `Line` 会把 `\n` 吞掉 | 排版时先按 `\n` 拆行 |
| 每来一个 token 就重建整段转录，再整帧 clone 一次 | §4.3 按块缓存，§5 合帧 |
| 在 `select!` 的分支里直接 await 读盘 | §3.1：所有调用 spawn 出去，结果以消息投回 |
| `FrameState` 由两方各填一半 | §4.1：只有 `view` 一个写者 |
| 键位层没有上下文概念，只能给每个模态起别名 | §6：绑定带上下文 |
| 契约里有一批字段从来没人产出 | 守卫：每个字段都必须被某张快照覆盖到 |

## 10. 待定

| 问题 | 倾向 |
|---|---|
| 项目层目录 | 已提 Agix#86，落地后改成 `.codix/` |
| 空闲超时的缺省值 | 30 分钟（`codix.yaml` 的 `instance.idle_timeout`），设为 `0` 表示不超时 |
| 缺省终端尺寸 | 160×45，写在 `config.toml`，可以设为 `off` |
| 宽屏下左右两栏的缺省比例 | 60 : 40，可调，并记住 |
| 实例开机自启 | P2：在手册里给出 systemd user unit 和 launchd plist 的示例 |

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
h.keys("Ctrl+R");                       // 进入提问框
h.snapshot("m04-ask-open");             // 与 tests/snapshots/m04-ask-open.txt 比较
h.keys("2 Enter");
assert_eq!(h.backend().calls().last(), Some(&Command::Answer { .. }));
```

鼠标的写法与按键相同，坐标是快照里的行和列：`h.click(col, row)`、`h.double_click(..)`、`h.drag((c0, r0), (c1, r1))`、`h.scroll(col, row, -3)`。也可以写成 `h.click_on("side.upper", "重构认证模块")`，按区域和文字定位，不受布局变化影响。

终端外壳的读写抽象成 `Term` trait，测试里用 `FakeTerm` 实现。启动时的尺寸协商可以测三种情况：终端照办、不响应、在 tmux 里不发请求。另外还要测退出时恢复原尺寸，以及鼠标捕获的开和关。

快照只记字符，不记样式。样式（高亮、颜色）需要验证时，在 L1 里断言 `FrameState` 上对应的字段。

快照的尺寸固定为三档：`80×24`、`120×40`、`200×50`。设置环境变量 `CODIX_UPDATE_SNAPSHOTS=1` 时重写快照；重写后的 diff 必须在提交里人工看过。

## 3. mock 后端

### 3.1 种子数据

`crates/mock/seeds/*.yaml`：一组可以复用的初始状态，包括会话（带若干轮历史）、任务树、goal、todo、流程定义（按 Harnix 的格式手写，方便以后对照）、运行、作业及其历史。

种子数据也包括实例列表：`MockInstances` 列出的实例覆盖运行中、已停止、残留三种状态，接入或新建一个实例时，就加载下表中的某个种子作为它的内容。

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
codix                                # 阶段 2 直接打开演示 demo（一个场景里什么都有）
codix --mock m11-flow-run            # 用某个测试场景启动真实终端界面
codix --mock m11-flow-run --speed 4  # 时间线加速 4 倍
codix --mock default                 # 只加载种子，不跑时间线
```

只有 mock 模式下，`/events` 才会打开**事件日志浮层**：滚动显示后端推来的每个事件和发出的每条命令，人工验收时用来对照"界面是不是如实反映了后端"。

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
| m09-session-tree | 3 层子 Agent + 各种任务，状态频繁变化；进入子 Agent、看 Shell 输出、发消息、停止 | S-04 S-05 S-06 C-16 · 交互 §4.1 §5.2 |
| m10-goal-todo | goal 自动开轮、受阻、暂停和恢复；todo 逐项完成 | S-02 S-03 · 交互 §5.2 |
| m11-flow-run | 一次运行走完：并行的站、搁置后放行、等确认、未达标后返工、暂停后恢复、取消 | F-02 F-03 F-04 F-05 · 交互 §5.3 |
| m12-flow-author | 选中流程定义左边不动、右下是流程设计；在用户会话里说着改流程，设计跟着变；`R` 起话头启动运行；查看定义 | F-07 F-08 · 交互 §4.1 §5.3 |
| m13-jobs | 简单作业和流程作业按时触发、失败、立即运行、启用和停用、从历史跳转 | J-01 至 J-04 · 交互 §5.4 |
| m14-attention | 多处同时等人（会话、站、受阻的 goal、失败的作业），用 ^R 依次处理，角标随之变化 | L-04 L-05 |
| m15-reconnect | 与实例断线 → 重连中 → 恢复（`Resynced` + 快照），列表变灰后恢复；实例被停止后回到实例选择 | I-09 · 交互 §9 |
| m16-storm | 每秒 1000 个 `TextDelta`，同时 50 个任务在更新：用计数断言重排和画帧的次数 | N-02 |
| m17-layout | 在 200 / 120 / 100 / 80 / 60 列之间切换；收起右栏；调整比例；单栏下来回切换 | L-02 L-03 N-01 · 交互 §2 |
| m18-errors | 一轮出错、命令失败、列表加载失败后重试 | 交互 §9 |
| m19-many | 用 `busy` 种子：长列表的滚动、过滤、选中保持不变 | S-01 F-01 J-01 |
| m20-first-run | 用 `empty` 种子：每个标签的空状态和引导文字 | 交互 §9 |
| m21-instances | 实例选择：运行中、已停止、残留、已被占用四种状态；新建、接入、启动、停止；接入被占用的实例时被拒；强制接入（二次确认）以及被踢一方的"已脱离"界面；空闲超时前的提示与自动脱离；之后重新接入；脱离和停止的区别；心跳丢失后实例断开连接 | I-01 至 I-06、I-10、I-11 · 交互 §2.2 §4.5 §9 |
| m22-mouse | 交互 §6 表中每一格都要点一遍：标签、列表、展开、工具块、提问选项、二次确认按钮、拖动分隔线、各区域的滚轮、关闭和恢复鼠标捕获；转录里拖选并复制（P1） | M-01 至 M-08 · 交互 §6 |
| m23-startup-size | 尺寸协商的三种情况：终端照办、不响应、在 tmux 里；退出时恢复原尺寸；配置成 `off` | L-06 · 交互 §2.1 |
| m24-footer-hint | 在每一种上下文、每一种浮层和二次确认下，分别在 60 / 80 / 120 / 160 列宽度检查底栏最右边都是 `^G 切块 · ^N 标签 · ^B 切栏 · ^X 退出 · ^H 帮助`；单栏模式下能切过去也能切回来。键位的其余用例（^G 一圈三格、单栏换栏、回左边进提问框，^N 轮换标签，^T 交还输入框，^O 看转录与各种离开都收回精简，转录里 `o` `[` `]`，^H 退格或帮助，兼容键，`/mouse` `/events`）都在 `crates/app/tests/keys.rs` | L-07 L-12 · 交互 §2.4 §3 §7 |
| m25-ownership | 顶栏与对话框标题右边覆盖每一种来源：用户、作业、流程站、子 Agent、分叉；窄宽度下的截断；会话被另一个实例占用时的提示与只读 | C-12 S-08 · 交互 §4.1 |

## 5. 守卫

| 守卫 | 内容 |
|---|---|
| 依赖方向 | `tui` 不依赖 `port` / `model`；`model` 不依赖 ratatui 的终端后端，也不依赖 tokio 运行时；`port` 不依赖 Agix 和 Harnix；`app` 不依赖 Agix |
| 键位 | 同一上下文里没有重复绑定；每个动作都有帮助文字；每个全局动作在 §6 的表里都有替代键或替代命令 |
| 字段覆盖 | 每个 `FrameState` 字段至少在一张快照里取到非缺省值，防止出现"契约里有、但没人产出"的字段 |
| 块首标记 | 只有 `model` 里那一个辅助函数可以读 `first_of_block`（用 grep 守卫） |
| 鼠标不另走逻辑 | 鼠标命中测试的结果只能是一个已有的动作，`update` 里没有专门处理鼠标的业务分支（只有命中测试和拖动状态） |
| 底栏保底 | 对每一种键位上下文渲染一次底栏，断言最后两项是"切换栏位"和"帮助"的实际绑定键 |

## 6. 阶段 2 的验收标准

1. `interaction.md` 里所有的 P0 交互都能在 mock 下操作出来。
2. 上面 25 个场景的 L3 用例全部通过；`cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` 全部通过。
3. `docs/manual-smoke-checklist.md` 已经写好，并在至少四种环境里逐项过完：一个 macOS 终端（尺寸协商、鼠标）、tmux（剪贴板、鼠标）、ssh 到 Linux、Linux 物理控制台或固定尺寸的终端（单栏切换、底栏保底提示）。
4. 维护者本人用 `codix --mock …` 亲手试过，确认交互没有问题。

四条都满足才进入阶段 3。

## 7. 阶段 3：真实后端

mock 那一套不动，真实后端按层另加用例，全部离线、不调模型：

| 层 | 测什么 | 怎么测 |
|---|---|---|
| `wire` | 分帧、按 `id` 配对乱序的回复、通知进 channel、连接断了在等的调用一律失败；不认识的 `kind` 跳过 | duplex 另一端的假服务端 |
| `host::hub` | 端口命令变成哪些 Agix 调用、帧变成哪些领域事件、推给谁（只推 Watch 着的会话的细节）；子 Agent 的帧与提问；作业工具与触发记录 | 脚本化的假引擎（`Engine` trait），通知直接塞进 channel |
| `host::jobs` | cron 的本地时区、存储与触发记录、补跑一次、改了就重排 | 注入时钟 + tokio 暂停时间 |
| `host::server` / `client` | 握手、占用、强制接入与被踢、脱离、心跳超时、停实例、事件排在回复前；断线重连；残留检测 | 临时目录里的 socket + 假 `Service` |
| `engine` | 真实嵌入的 Agix + `Hub`：建会话、跑一轮、这一轮有结局 | 没有凭据时那一轮以 `no_credentials` 结束，所以离线也能跑 |

调真实模型的用例标 `#[ignore]`，带上凭据手动跑：`source .env && cargo test -p engine -- --ignored`。
把实例进程、接入与模型整条接起来的用例见 §8。

## 8. LIVE：真实实例 + 真实模型

§7 的各层都是离线的，各自替掉了相邻的一层。LIVE 把整条路接起来：真实的 `codix host` 进程、嵌在里面
的 Agix、真实的模型，从 TUI 走的那条路接入——`SocketInstances` 拉起与接入，`SocketBackend` 收发
`port` 的命令与事件。断言大多落在端口上（回复、事件）和实例目录里的文件上；`live_tui.rs` 再往上一层，
用真的 `app::Core`（键位、model、视图）接真实实例，按键、看界面状态与屏幕。

**只测 Codix 自己做的事。** Agix 的特性（工具怎么跑、权限怎么判、goal 怎么推进、压缩……）由 Agix
自己的 LIVE 测，这里不重复。这里测的是 Codix 加在它外面的那一层：实例的锁、发现文件与 `meta.json`、
单一接入与心跳、帧到领域事件的翻译与路由、会话归属与名字（`sessions.json`）、作业的调度与触发记录、
宿主工具。用例要靠模型做某件事时，话说得短而确定（"只回复 OK"），并且点名引擎真有的工具（Agix 的 Bash
只在前台跑，后台命令要点名 Monitor），断言只看 Codix 的翻译：任务作为没有会话的任务出现、输出取得到、
停得掉，这一轮怎么收尾是模型的事。

| 文件 | 覆盖 |
|---|---|
| `live_instance.rs` | 新建、列出、发现文件与 `meta.json`、目录 0700 与 socket 0600、同名第二个实例拿不到锁、同一项目的第二个实例被拒并报出占着它的实例、停掉再照 `meta.json` 拉起、实例被杀后看护进程把它拉起来（`list` 写出重启次数）、连看护进程一起被杀后列为残留并清理 |
| `live_attach.rs` | `Hello` 与能力位、被占用、强制接入踢人、脱离放开接入、空闲脱离、心跳超时断开后重连、实例被停与 `StopInstance` |
| `live_sessions.rs` | 只列本项目的会话、受信项目的会话放在项目里（不受信的在全局，都跨重启列得出）、第一句话做名字、改名与关闭跨重启保留、换模型、命令表、Watch 快照的历史与 todo、拒绝逐轮权限模式 |
| `live_turns.rs` | 文字与用量、忙时退回、插话送达、排队、中断、权限与模型提问的作答、侧问分叉、goal 的事件与快照、后台任务的输出与停止、子 Agent 的侧链会话与给它发消息 |
| `live_jobs.rs` | 增删改与启停（定义进 `.codix/jobs.json`、启停进本机的 `jobs-state.json`）、立即运行触发的会话与结局、触发记录落盘、重启后没跑完的那次记为中断、错过的触发只补一次、停下正在跑的那一次记成手动停下、codix.yaml 给的超时到了被打断记成超时、停实例与崩溃留下的那次记成"实例停了"、一次失败的触发经配置的命令出口送出（TestNotify 回每个出口的结果）。作业定义要项目受信才读写，用例用 `start_trusted` |
| `live_flows.rs` | 流程：内置示例流程；项目没受信时 `flow_create` 存不进去、`/trust` 之后存进项目自己的流程包；用户会话经 `flow_validate` / `flow_create` 写流程；一次运行从头跑到尾、主会话的归属；`workspace: worktree` 的运行在仓库根的 `.wkts/<运行>/`、主仓库看不到它与项目状态；有运行时 `flow_update` 被拒并列出运行，取消后才能改；运行里的会话动不了别的流程；流程作业触发一次运行；用户会话用 `job_validate` / `job_save` / `job_run` 在对话里建出流程作业并换主题跑一次；库的一致性套件对着 Codix 的 `SessionHost`（带凭据把测试进程再起一遍，引擎嵌在里面） |
| `live_tui.rs` | 界面经真实实例：经界面新建并接入、底栏那几项；^N 轮换标签（流程标签里是实例给的示例流程）、^G 一圈三格、单栏时换栏；^T、^H；`/trust` 的回复；^X 后台运行后实例还在、再接回来。草稿上第一句话建出会话并跑完一轮，^O 看转录，^G 到右上选中的就是它，`/rename` 经实例改名。真实的权限询问：^G 走开、^R 找回、转回左边进提问框、Enter 批准后工具真的跑了 |
| `live_tools.rs` | 宿主工具：用户会话经 `job_*` 增改停删作业、`session_rename` 改自己的名；查看不问、改东西经权限询问；`#n` 与 ID；作业触发的会话动不了别的作业 |

**怎么跑**：`scripts/live.sh`（全部）、`scripts/live.sh live_jobs`（一个文件）、`scripts/live.sh live_jobs
catch_up`（再按名字过滤）。凭据取仓库根的 `.env`（worktree 里往上找；`CODIX_LIVE_ENV` 可指定），要有
`ANTHROPIC_AUTH_TOKEN`（或 `ANTHROPIC_API_KEY`）与 `ANTHROPIC_MODEL`，读不到就失败并说明。凭据只交给
实例进程的环境（`SocketInstances::with_env`），不进测试进程、不打印。同时起的实例数用 `LIVE_THREADS`
控制（缺省 4）。用例全标 `#[ignore = "live"]`，`./scripts/check.sh` 只编译不跑。

**harness**（`crates/codix/tests/live/mod.rs`）：

- `Live`：一个用例的世界——/tmp 下短路径的 `CODIX_HOME`（socket 路径有 104 字节的上限）、一个 git
  示例项目、写好模型名的 Agix `settings.json`、可选的 `codix.yaml` 与只给实例的环境变量。`start`、
  `attach`、`stop`、`restart`、`start_again`；`read("meta.json")` 读实例目录的文件、`read_state("sessions.json")`
  读项目状态 `state/codix/` 里的；`Drop` 时
  不论成败停掉它起的所有实例。
- `tui::Tui`（`live/tui.rs`）：真的界面接真实实例。`Tui::create` / `Tui::attach` 经界面新建或接入；
  `keys("^G ^N")`、`type_str`、`say`（打字加回车）；`until(说明, 超时, |t| …)` 用墙上时间等，一边收实例的
  消息一边按 model 给的时刻 Tick，等不到就附上当时的屏幕；`text`、`last_line`、`transcript`、`idle`。
- `Client`：一个接入，相当于一个 TUI。`ok(cmd)`（失败就让用例失败并说是哪条命令）、`call(cmd)`；
  `wait_event(说明, 超时, |e| …)` 从游标往后等，等不到时附上最近 30 条事件；`new_session`、`watch`、
  `turn(会话, 话) -> TurnOutcome`（文字、工具、结果、提问、用量、结局）、`wait_prompt`、`wait_upsert`。
  心跳缺省像开着的 TUI 那样每 10 秒报一次，`heartbeat(Beat::None)` 停掉；提问缺省留给用例自己答，
  `answering(Answering::Auto)` 改成权限一律允许、问题选第一项（Watch 快照里悬着的也答）。

**加用例**：功能归哪个文件就加在哪个文件，或新开一个 `live_<功能>.rs`（`mod live;` 引入 harness，
`scripts/live.sh` 自动带上）。等事件，不比时长；跑不稳先找原因，不放宽断言。为可测性给实例加的
开关只走环境变量，名字以 `CODIX_TEST_` 开头（现有 `CODIX_TEST_HEARTBEAT_TIMEOUT_MS`），不进 `codix.yaml`。

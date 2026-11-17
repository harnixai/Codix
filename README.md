# Codix

跑在终端里的 agent 工作台，引擎是 [Agix](https://github.com/openatta/Agix)。左边是对话区，右边是三个标签：会话（主 Agent / 子 Agent / 任务）、流程、作业。可以本地用，也可以远程用，不限定于编程场景。

正在从头开发。开发约定见 [CLAUDE.md](CLAUDE.md)，需求与设计见 `docs/`。

```sh
git clone --recurse-submodules <repo>
git -C agix checkout main
```

阶段 2 的界面接的是 mock 后端，直接运行就是演示：

```sh
cargo run -p app          # 演示 demo：会话、任务树、goal、流程运行、作业都在里面，约一分钟内自己动起来
```

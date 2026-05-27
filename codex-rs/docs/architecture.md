# Codex CLI 架构文档

> 本文面向贡献者，自上而下梳理仓库的代码组织、关键 crate 的职责，以及一次会话从用户输入到模型输出的端到端数据流。配合 `AGENTS.md`、`../app-server/README.md`、`../../docs/config.md` 阅读效果更佳。

---

## 1. 项目定位

Codex 是 OpenAI 推出的 **本地编码代理（coding agent）**。同一个 `codex` 二进制可以以多种形态运行：

- **交互式 TUI**（默认）：在终端里以聊天界面驱动代理；
- **`codex exec`**：非交互/脚本模式，单次跑完即退出；
- **`codex app-server`**：以 JSON-RPC 2.0 服务的形式被 VS Code 扩展、桌面 App、远端客户端调用；
- **`codex mcp-server`**：把 Codex 自己以 [MCP](https://modelcontextprotocol.io/) 服务暴露给外部 Agent；
- **`codex mcp` / `codex plugin`**：管理 *外部* MCP 服务与本地插件；
- **`codex cloud`**：与 OpenAI 云端任务队列对接；
- 还有 `codex resume`、`codex fork`、`codex doctor`、`codex sandbox`、`codex apply` 等运维/调试子命令。

完整子命令枚举见 `codex-rs/cli/src/main.rs:117` 的 `enum Subcommand`。

---

## 2. 仓库顶层布局

```
codex/
├── codex-rs/        # Rust 工作区，主程序所在地（≈80 个 crate）
├── codex-cli/       # npm 包 @openai/codex 的 JS 包装器（仅做平台分发）
├── sdk/             # 客户端 SDK（Python、TypeScript、python-runtime）
├── docs/            # 文档（你正在读的就在这里）
├── scripts/         # 构建/安装/调试脚本
├── tools/           # 仓内开发工具（lint、检查器等）
├── third_party/     # 第三方代码 / 许可声明
├── patches/         # 对上游依赖的补丁
├── flake.nix        # Nix 开发环境
├── BUILD.bazel      # 顶层 Bazel
├── MODULE.bazel     # Bzlmod 模块定义
├── justfile         # 任务编排（`just codex`、`just test`、`just fmt`…）
├── package.json     # pnpm workspace 元信息
└── AGENTS.md        # 贡献者必读：约定与红线
```

`codex-rs/Cargo.toml` 列出全部成员 crate，所有 crate 名都以 `codex-` 为前缀（详见 `AGENTS.md` 第 1 节）。

---

## 3. 构建与任务编排

仓库同时维护 **Cargo + Bazel** 两套构建系统：

| 系统 | 用途 |
|---|---|
| **Cargo** (`codex-rs/Cargo.toml`) | 日常开发、`just test/fix/fmt`、本地编译。`workspace.dependencies` 集中管版本 |
| **Bazel** (`BUILD.bazel`、`MODULE.bazel`) | 跨平台 hermetic 构建、CI、二进制发布。改 `Cargo.toml`/`Cargo.lock` 后需要 `just bazel-lock-update` |
| **pnpm** (`pnpm-workspace.yaml`) | 管理 `codex-cli/`、`sdk/typescript/` |
| **uv** (`sdk/python/pyproject.toml`) | 管理 Python SDK |

**`justfile`** 是入口聚合器，工作目录固定在 `codex-rs/`：

- `just codex [ARGS]` → `cargo run --bin codex -- ...`
- `just exec [ARGS]`、`just tui-with-exec-server`、`just app-server-test-client`
- `just fmt` / `just fix [-p crate]` / `just clippy` / `just test [-p crate]`
- `just bazel-lock-update` / `just bazel-lock-check` / `just argument-comment-lint`

详见 `justfile`。

---

## 4. `codex` 二进制入口

唯一的可执行产物是 `codex-cli` crate 编译出来的 `codex`（`codex-rs/cli/Cargo.toml:8`）。它是一个 **multitool** —— 用 clap 的 `subcommand_negates_reqs` 把所有子命令塞进同一个二进制。

`main.rs` 的关键骨架：

1. **`arg0_dispatch_or_else`**（`codex-rs/arg0`）：根据 `argv[0]` 把进程伪装成 `codex-linux-sandbox` 这类内部辅助二进制 —— 一份代码、多副人格。
2. **`MultitoolCli::parse()`**：解析 `--enable/--disable` 等全局开关、子命令、TUI 默认参数。
3. **`match subcommand { ... }`**：把控制权移交给各业务 crate（`codex_exec::run_main`、`codex_mcp_server::run_main`、`codex_app_server::run_main_with_transport_options`、`codex_tui::run_main`、`codex_cloud_tasks::run_main` …）。

子命令到承载 crate 的映射：

| 子命令 | 承载 crate |
|---|---|
| 默认（无子命令） | `codex-tui` |
| `exec` / `review` | `codex-exec` |
| `app-server` | `codex-app-server` (+ `codex-app-server-daemon` 管子进程) |
| `app-server proxy` / `stdio-to-uds` | `codex-stdio-to-uds` |
| `app-server generate-ts/json-schema` | `codex-app-server-protocol` |
| `mcp-server` | `codex-mcp-server` |
| `mcp` | `cli/src/mcp_cmd.rs` → `codex-mcp` |
| `plugin` / `marketplace` | `cli/src/plugin_cmd.rs`、`marketplace_cmd.rs` |
| `cloud` | `codex-cloud-tasks` |
| `login` / `logout` | `codex-login` + `codex-cli` 辅助函数 |
| `sandbox` | 平台特化：`codex-linux-sandbox` / Seatbelt / Windows Sandbox |
| `apply` | `codex-chatgpt::apply_command` |
| `execpolicy check` | `codex-execpolicy` |
| `resume` / `fork` | TUI + `codex-rollout` |
| `responses-api-proxy` | `codex-responses-api-proxy` |
| `exec-server` | `codex-exec-server` |
| `features list/enable/disable` | `codex-features` |
| `doctor` | `cli/src/doctor.rs` |

---

## 5. 分层与 crate 分组

```
┌─────────────────────────────────────────────────────────────────────┐
│  Front-ends                                                          │
│  ┌────────────┐  ┌──────────────┐  ┌─────────────────┐  ┌──────────┐│
│  │ codex-tui  │  │ codex-exec   │  │ codex-app-server│  │ codex-mcp││
│  │ (TUI)      │  │ (CLI batch)  │  │ (JSON-RPC 2.0)  │  │  -server ││
│  └─────┬──────┘  └──────┬───────┘  └────────┬────────┘  └─────┬────┘│
│        └────────────────┴───────────────────┴─────────────────┘     │
│                                 │                                    │
│                    ┌────────────▼─────────────┐                      │
│                    │  codex-core (Agent loop) │                      │
│                    │  Thread / Turn / Item    │                      │
│                    │  Context / Skills / MCP  │                      │
│                    │  Approvals / Compaction  │                      │
│                    └────────────┬─────────────┘                      │
│         ┌──────────────┬────────┼─────────┬──────────────┐           │
│   ┌─────▼─────┐  ┌─────▼─────┐ │  ┌──────▼─────┐  ┌─────▼─────┐    │
│   │ Protocol  │  │  Model    │ │  │ Sandboxing │  │  Storage  │    │
│   │ (wire)    │  │ Providers │ │  │ (3 OSes)   │  │ (rollout) │    │
│   └───────────┘  └───────────┘ │  └────────────┘  └───────────┘    │
│                       ┌────────▼─────────┐                          │
│                       │ Tools / ext API  │                          │
│                       │ Skills / Plugins │                          │
│                       │ Hooks / Connectors│                         │
│                       └──────────────────┘                          │
└─────────────────────────────────────────────────────────────────────┘
```

下面按职责分组列出主要 crate（一行描述）。完整清单见 `codex-rs/Cargo.toml`。

### 5.1 入口与前端

| Crate | 职责 |
|---|---|
| `codex-cli` | 二进制入口；clap 子命令路由；登录/认证流程入口（`run_login_*`）|
| `codex-tui` | Ratatui 实现的交互终端 UI；样式约定见 `codex-rs/tui/styles.md` |
| `codex-exec` | 非交互执行（脚本/CI 友好），含 `Review` 子流程与 `event_processor_*`（人类可读 vs JSONL 输出）|
| `codex-exec-server` | 独立 exec 服务，可挂载到远端环境 |
| `codex-app-server` | 主 JSON-RPC 2.0 服务，被 VS Code 扩展和桌面 App 消费 |
| `codex-app-server-daemon` | 本地 daemon 生命周期（start/restart/stop/bootstrap）|
| `codex-app-server-protocol` | 协议类型；可导出 TS / JSON Schema |
| `codex-app-server-transport` | stdio / WebSocket / Unix socket 三种传输 |
| `codex-app-server-client` / `-test-client` | 内部客户端 + 测试夹具 |
| `codex-mcp-server` | 把 Codex *自身* 暴露为 MCP server |

### 5.2 核心运行时

| Crate | 职责 |
|---|---|
| `codex-core` | **代理核心**：`CodexThread`、`TurnContext`、Session 循环、上下文管理、紧凑化（compact）、apply-patch、agents.md 加载、客户端胶水、`codex_delegate` 子代理 |
| `codex-core-api` | 暴露给外部使用的稳定 API 子集 |
| `codex-core-plugins` / `codex-core-skills` | 内置插件 / 内置 Skill |
| `codex-protocol` | **线协议类型**：`approvals`、`exec_output`、`items`、`mcp`、`network_policy`、`permissions`、`plan_tool`、`request_permissions`、`session_id`、`shell_environment`、`thread_id`、`user_input`、`openai_models` 等。其他层都依赖它做序列化 |
| `codex-tools` | 工具系统脚手架 |
| `codex-hooks` | 生命周期 hook（SessionStart、CodexEvent 等）|

> **关于 `codex-core` 的红线**：`AGENTS.md` 反复强调“抵制向 codex-core 加代码”。新增能力应优先放新 crate（或现有 ext / utils）。

### 5.3 模型与鉴权

| Crate | 职责 |
|---|---|
| `codex-api` / `codex-backend-client` | OpenAI / 后端 HTTP 客户端 |
| `codex-model-provider` / `-info` | 可插拔 model provider 抽象 + 元数据 |
| `codex-models-manager` | 已捆绑模型目录与按需刷新（`RefreshStrategy`）|
| `codex-chatgpt` | ChatGPT 特化（包含 `apply_command`）|
| `codex-ollama` / `codex-lmstudio` | 本地 LLM 后端适配 |
| `codex-login` | `AuthManager` / `CodexAuth`，统一 ChatGPT / API Key / Device Code / Access Token |
| `codex-aws-auth` | AWS SigV4 |
| `codex-keyring-store` | 系统 keyring 凭据存储 |
| `codex-secrets` | 内存中的 secret 处理与脱敏 |
| `codex-responses-api-proxy` | Responses API 本地代理（`codex responses-api-proxy`）|
| `codex-codex-api` / `codex-experimental-api-macros` | OpenAI Responses API 客户端 + 派生宏 |
| `codex-realtime-webrtc` | Realtime 语音/视频会话 |

### 5.4 沙箱与策略

跨 OS 抽象 + 三套底层实现：

| Crate | 职责 |
|---|---|
| `codex-sandboxing` | 平台无关的 sandbox 接口 |
| `codex-linux-sandbox` | Linux Landlock + seccomp（编译为内嵌的 `codex-linux-sandbox` 子二进制，通过 arg0 分派）|
| `codex-bwrap` | Bubblewrap 用户命名空间封装 |
| `codex-windows-sandbox-rs` | Windows Sandbox 容器集成 |
| `codex-process-hardening` | 限制 coredump / ptrace 等 |
| `codex-execpolicy` / `-legacy` | 命令执行白名单/审批策略；`codex execpolicy check` 暴露 CLI |
| `codex-network-proxy` | 网络策略与代理 |

> 同时务必看 `AGENTS.md` 关于 `CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR` 和 `CODEX_SANDBOX_ENV_VAR` 的红线 —— **不要新增或修改相关代码**。背景见 `../../docs/sandbox.md`。

### 5.5 状态与存储

| Crate | 职责 |
|---|---|
| `codex-rollout` | 会话 JSONL 持久化（`$CODEX_HOME/sessions/`）、重放、紧凑化 |
| `codex-rollout-trace` | 缩减状态 trace（调试用）；`codex debug trace-reduce` 用它 |
| `codex-thread-store` | Thread 元数据（SQLite）|
| `codex-message-history` | 历史 item 检索 |
| `codex-state` | `StateRuntime` + `state_db_path`（应用整体状态库）|
| `codex-agent-graph-store` | Agent 依赖图存储 |
| `codex-agent-identity` | Agent 身份/元信息 |
| `codex-memories` (在 `ext/memories`) + `memories-read/-write` | 长期记忆读写 |

### 5.6 文件、Shell 与 Git

| Crate | 职责 |
|---|---|
| `codex-file-system` / `codex-file-watcher` | 文件 I/O 抽象 + FSEvents/inotify |
| `codex-file-search` | 内容/符号搜索（BM25）|
| `codex-apply-patch` | 解析并应用 diff |
| `codex-git-utils` | Git 分支/diff/remote 信息 |
| `codex-shell-command` / `codex-shell-escalation` | Shell 检测与权限提升 |
| `codex-terminal-detection` / `codex-ansi-escape` | 终端能力探测 + ANSI 工具 |

### 5.7 扩展机制（`codex-rs/ext/`）

`ext/` 是 **稳定的扩展 API + 几个内置扩展示例**：

- `ext/extension-api`（`codex-extension-api`）：基于 async trait 的扩展契约，依赖 `codex-protocol` + `codex-tools`，是第三方扩展应该写到的接口；
- `ext/goal`：目标跟踪（token / 时间预算）；
- `ext/guardian`：审批子代理（评估命令风险）；
- `ext/memories`：长期记忆扩展。

其他扩展相关 crate：

| Crate | 职责 |
|---|---|
| `codex-plugin` | 插件清单解析与加载 |
| `codex-skills` | 用户自定义 Skill（基于 markdown 的 `SKILL.md`）|
| `codex-core-skills` / `codex-core-plugins` | 内置实现 |
| `codex-connectors` | 第三方服务接入（Slack、GitHub 等）|
| `codex-hooks` | 配置驱动的生命周期 hook |
| `codex-collaboration-mode-templates` | Plan/Implement/Review 等协作模式模板 |

### 5.8 云、协作与远端

| Crate | 职责 |
|---|---|
| `codex-cloud-tasks` / `-client` / `-mock-client` | 远端任务队列 + Mock |
| `codex-cloud-requirements` | 云端调用所需的能力/特性门控 |
| `codex-external-agent-migration` / `-sessions` | 与历史外部 agent 会话迁移/对接 |
| `codex-backend-client` | 后端 API 客户端 |

### 5.9 工具/辅助 crate

| Crate | 职责 |
|---|---|
| `codex-utils-*`（`absolute-path`、`pty`、`image`、`string`、`cache`、`cli`、`approval-presets`、`fuzzy-match`、`stream-parser`、`template`…）| 小而专的工具集 |
| `codex-async-utils` | Tokio 辅助、channel 封装 |
| `codex-config` | 加载 `config.toml`、profile、feature flag |
| `codex-features` | 特性阶段（`UnderDevelopment / Experimental / Stable / Deprecated / Removed`）+ `FEATURES` 注册表 |
| `codex-otel` | OpenTelemetry instrumentation |
| `codex-analytics` / `codex-feedback` | 遥测与用户反馈 |
| `codex-install-context` | 检测安装方式（npm/brew/standalone/cargo …）影响 `codex update` |
| `codex-arg0` | argv[0] 路由（多人格二进制的关键）|
| `codex-uds` / `codex-stdio-to-uds` | Unix domain socket 工具 |
| `codex-test-binary-support` | 测试用辅助二进制 |
| `codex-code-mode` | 代码模式开关 |
| `codex-response-debug-context` | 模型响应调试上下文 |
| `codex-v8-poc` | V8 JS 运行时 PoC（实验性）|

---

## 6. 协议层：`codex-protocol`

`codex-protocol` 是 **跨层共享的“词汇表”**。看 `codex-rs/protocol/src/` 即可了解 Codex 把世界抽象成什么：

- 输入：`user_input.rs`、`request_user_input.rs`、`request_permissions.rs`
- 输出/事件：`items.rs`、`exec_output.rs`、`plan_tool.rs`
- 安全：`approvals.rs`、`permissions.rs`、`network_policy.rs`、`mcp_approval_meta.rs`
- 标识：`thread_id.rs`、`session_id.rs`、`account.rs`、`agent_path.rs`
- 模型：`models.rs`、`openai_models.rs`、`dynamic_tools.rs`
- 环境：`shell_environment.rs`、`config_types.rs`
- 关键聚合：`protocol.rs`（含 `SessionSource`、`AskForApproval` 等枚举）、`prompts/`（系统提示模板）

**任何跨 crate 的传输类型都应该住在这里**；这是不增长 `codex-core` 的关键策略之一。

---

## 7. app-server：JSON-RPC 2.0 网关

`codex-app-server` 把核心运行时包成一个 **类 MCP 的 JSON-RPC 2.0 服务**（详见 `codex-rs/app-server/README.md`）。

**三大原语**（来自 README）：

- **Thread**：一段会话；每个 thread 包含多个 turn，落盘成 JSONL。
- **Turn**：一次完整对话回合：用户输入 → 模型生成 → 副作用。
- **Item**：turn 内的离散单元（user message、agent reasoning、agent message、shell command、file edit……）。

**传输**：`--listen stdio:// | unix:// | ws://IP:PORT | off`。Unix socket 走 HTTP Upgrade 切换到 WebSocket 帧；WebSocket 监听器还顺带 `/healthz` 与 `/readyz`。

**生命周期**：连接建立 → `initialize` + `initialized` 握手 → `thread/start | thread/resume | thread/fork` → `turn/start` → 流式接收 `item/started`、`item/agentMessage/delta`、`item/completed`、`turn/completed` 等通知。

**背压**：内部用有界队列；过载时返回 `-32001 "Server overloaded; retry later"`。

**关键 API 类目**：thread 生命周期、turn 控制（含 `turn/steer`、`turn/interrupt`）、`item/*` 流、`command/exec`、`process/spawn`、`fs/*`、`plugin/*`、`skills/*`、`model/list`、`experimentalFeature/list`、`mcpServer/tool/call`、`thread/realtime/start`。

**Schema 生成**：

```bash
codex app-server generate-ts --out DIR
codex app-server generate-json-schema --out DIR
```

—— 由 `codex-app-server-protocol` 完成，输出与当前二进制版本一一对应。

---

## 8. MCP 集成的两个方向

Codex 同时是 **MCP 客户端和服务端**：

| 方向 | Crate | 命令 |
|---|---|---|
| Codex 调用 *外部* MCP 工具 | `codex-mcp`、`codex-rmcp-client`、`codex-rs/codex-mcp/src/mcp_connection_manager.rs` | `codex mcp …`（管理外部服务器配置）|
| Codex *被* 外部 Agent 调用 | `codex-mcp-server` | `codex mcp-server`（stdio）|

> `AGENTS.md` 特别指出：MCP 工具调用相关的工具/调用变更，应该走 `codex-mcp/src/mcp_connection_manager.rs`，**最小化改动半径**。

---

## 9. TUI

`codex-tui` 是默认前端（无子命令时被路由）。模块组织充分体现了 `AGENTS.md` 里“**避免巨型模块**”的约束：`tui/src/app/`、`bottom_pane/`、`chatwidget/` 等被显式拆成子目录。规模红线：单文件 800 LoC 是上限。

样式约定（`codex-rs/tui/styles.md`）：

- 优先 Ratatui 的 `Stylize` trait（`"text".bold()` 而非手动 `Style`）；
- `"text".into()` 优于 `Span::from(...)`；
- **禁止使用 `.white()`** —— 留默认前景；
- 计算出的 `Style` 才用 `Span::styled`。

实际进入流程的入口在 `codex-rs/tui/src/lib.rs`（`run_main`）；交互完成后通过 `AppExitInfo`（`exit_reason`、`update_action`、`thread_id`、`thread_name`、`token_usage`）回到 `cli/src/main.rs::handle_app_exit`。

---

## 10. 关键数据流：一次 `codex` turn

以默认 TUI / 用户在终端敲入 `codex` 为例：

```
┌─────────┐
│ user @  │
│ tty     │  ① 进程启动
└────┬────┘
     ▼
┌──────────────────────────┐
│ codex (codex-cli)        │  ② arg0_dispatch_or_else 决定人格
│ - clap parse             │  ③ MultitoolCli::parse → 默认子命令
│ - run_interactive_tui    │
└────────────┬─────────────┘
             ▼
┌──────────────────────────┐
│ codex-tui (Ratatui)      │  ④ 渲染 + 等待输入
│ - ChatComposer           │  ⑤ 用户回车 → UserInput
└────────────┬─────────────┘
             ▼
┌──────────────────────────┐
│ codex-core               │  ⑥ 新建/继续 CodexThread
│ - TurnContext            │  ⑦ 加载 agents.md / skills /
│ - context_manager        │     plugins / memories
│ - codex_thread.run_turn  │  ⑧ 注入系统提示 + 历史
└──┬──────┬────────────┬───┘
   │      │            │
   │      │            ▼
   │      │   ┌───────────────────┐
   │      │   │ codex-protocol /  │  ⑨ 序列化 Items
   │      │   │ codex-api/        │  ⑩ 调用 Responses API
   │      │   │ codex-model-...   │     (流式返回 SSE)
   │      │   └─────────┬─────────┘
   │      │             │
   │      │             ▼
   │      │   ┌───────────────────┐
   │      │   │ Streaming items   │  ⑪ agentMessage/delta、
   │      │   │ → core → tui      │     toolCall、commandExec…
   │      │   └─────────┬─────────┘
   │      ▼             ▼
   │  ┌────────────────────────────┐
   │  │ Tool / shell execution     │  ⑫ codex-tools dispatch
   │  │ - sandboxing (Landlock/    │     → guardian 审批 (ext)
   │  │   Seatbelt/WinSandbox)     │     → execpolicy 校验
   │  │ - apply-patch              │     → 真正运行命令
   │  └────────────┬───────────────┘
   ▼               ▼
┌──────────────────────────┐
│ codex-rollout            │  ⑬ 每个 item 追加写入 JSONL
│ $CODEX_HOME/sessions/    │     供 resume/fork/replay 使用
└────────────┬─────────────┘
             ▼
┌──────────────────────────┐
│ AppExitInfo → cli        │  ⑭ 打印 token usage + resume 提示
└──────────────────────────┘
```

在 **app-server 形态** 下，③→⑫ 之间多了一层 JSON-RPC：客户端发起 `turn/start`，服务端把 `item/*` 通知以推送的方式回流给客户端（VS Code、桌面 App、Python SDK）。

---

## 11. 配置与特性

- **配置加载**：`codex-config` + `codex-core::config::ConfigBuilder`。`config.toml` 位置由 `find_codex_home` 决定，支持 profile 叠加。Schema 在 `codex-rs/core/config.schema.json`，改 `ConfigToml` 后 **必须** 跑 `just write-config-schema`（`AGENTS.md`）。
- **Feature flag**：`codex-features` 的 `FEATURES` 全局表 + `Stage` 枚举。CLI 暴露：

  ```bash
  codex features list
  codex features enable <key>
  codex features disable <key>
  codex --enable <key> ...      # 等价于 -c features.<key>=true
  ```

- **Hooks**：`codex-hooks` 让 `config.toml` 注入 `SessionStart`、`CodexEvent` 等钩子，由 harness 而非模型驱动（参考 `../../docs/skills.md`、`update-config` skill）。

---

## 12. 分发与 SDK

### npm 包：`codex-cli/`

`codex-cli/package.json` 是 `@openai/codex` 的元信息，**不包含业务逻辑**：

- `bin/codex.js` 是一个 ESM 启动器；
- 它在运行时探测平台（`darwin-arm64`、`linux-x64-musl`、`win-x64` …），从平台特化的 npm 依赖（如 `@openai/codex-linux-x64`）里解析出 Rust 二进制，再用 `inherited stdio` spawn 出来；
- 这样用户可以 `npm i -g @openai/codex` 一键跨平台安装。

### SDK：`sdk/`

| 目录 | 内容 |
|---|---|
| `sdk/python/` | `openai-codex` Python SDK，Pydantic 模型 + JSON-RPC 2.0 stdio；模型与捆绑二进制版本对齐 |
| `sdk/python-runtime/` | Python SDK 的运行时支撑 |
| `sdk/typescript/` | TS SDK |

`codex-rs/scripts/` 下还有用于把 SDK / npm 包打包发版的辅助脚本。

### 自助分发

`scripts/install/install.sh` 与 `install.ps1` 是面向最终用户的下载器（拉 GitHub Release）。

仓内贡献者本地编译并安装可以用 `scripts/build-and-install-local.sh`（直接 `cargo build --release --bin codex` + 拷到 `~/.local/bin`）。

---

## 13. 测试与质量

`AGENTS.md` 明确：

- **不要** `cargo test` 直接跑，统一走 `just test`；
- 改动局部就 `just test -p codex-<crate>`；
- 改了 `common/core/protocol` 才考虑跑全量；
- 完成后 `just fmt`，并按需 `just fix -p <crate>`；
- `--all-features` 慎用，会撑爆 `target/`；
- Rust 锁经常慢，**别 kill Cargo 进程**。

工具：

- `tools/argument-comment-lint/`：对位置传递 `None / bool / 数字字面量` 的 lint，等价 `just argument-comment-lint`；
- `cargo-insta`：快照测试；
- `cargo-nextest`：作为默认测试 runner（`just test` 已封装）；
- 各 crate 的 `BUILD.bazel`：Bazel 下需要显式把 `include_str!/include_bytes!` 的依赖声明到 `compile_data / build_script_data / data`。

---

## 14. 阅读路径建议

1. **想理解整体**：本文件 → `codex-rs/app-server/README.md` → `codex-rs/cli/src/main.rs`。
2. **想读核心循环**：`codex-rs/core/src/codex_thread.rs`、`codex_delegate.rs`、`context_manager/`、`compact*.rs`。
3. **想读协议**：`codex-rs/protocol/src/protocol.rs`、`items.rs`、`approvals.rs`。
4. **想加扩展**：`codex-rs/ext/extension-api/src/lib.rs` + 现有 `ext/goal` / `ext/guardian` / `ext/memories` 三个例子。
5. **想加子命令**：`codex-rs/cli/src/main.rs:117` 起步，仿照已有分支。
6. **想加工具**：先看 `codex-rs/tools/`；MCP 类工具走 `codex-mcp/src/mcp_connection_manager.rs`。
7. **想理解 sandbox**：`../../docs/sandbox.md` + `../sandboxing/` + `../linux-sandbox/`。
8. **想加模型 provider**：`codex-rs/model-provider/` + 看 `ollama` / `lmstudio` 两个现成实现。

---

## 15. 设计原则速览

提炼自 `AGENTS.md`、各 README 与已落地代码：

1. **`codex-core` 是负债，不是资产** —— 优先放新 crate；
2. **跨层共享的类型放 `codex-protocol`**；
3. **模块拆分**：单文件目标 < 500 LoC，硬上限 ≈ 800 LoC，超就拆模块/拆 crate；
4. **避免过早抽象**：不为只调一次的方法建 helper；不写没有当前使用者的 API；
5. **trait async**：用 RPITIT + `Send`，不用 `#[async_trait]`、不用 `#[allow(async_fn_in_trait)]`；
6. **`match` 默认穷尽** —— 拒绝 wildcard；
7. **API 不暴露 `bool` / 裸 `Option` 参数** —— 用 enum / newtype / 命名方法；
8. **关于 `CODEX_SANDBOX*` 环境变量是只读领域** —— 谁都不许新增。

---

> 维护提示：本文档假设当前 `dev` 分支（commit `3936ed221d` 时）。crate 拓扑变动时请同步本文件的 §5；新增子命令时请同步 §4 的映射表。

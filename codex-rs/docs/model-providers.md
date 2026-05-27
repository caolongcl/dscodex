# 接入第三方模型

> 本文说明如何让 Codex 使用 OpenAI 之外的模型。所有事实出自 `codex-rs/model-provider-info/src/lib.rs` 与 `codex-rs/protocol/src/config_types.rs`，配 `config.md`、`example-config.md`（位于仓库根 `docs/`）阅读。

---

## 1. 概念

Codex 把"模型来源"抽象成 **provider**。一个 provider 对应一个 OpenAI Responses API 兼容的端点（或 AWS Bedrock 这类有专门客户端的特殊情况）。每个 provider 在 `model-provider-info/src/lib.rs:84` 的 `ModelProviderInfo` 中定义。

启动时 `built_in_model_providers()`（`lib.rs:409`）注册 4 个内置 provider：

| ID | 用途 |
|---|---|
| `openai` | OpenAI / ChatGPT 默认 |
| `amazon-bedrock` | AWS Bedrock（仅允许覆盖 `aws.profile` / `aws.region`）|
| `ollama` | 本地 Ollama（默认 `localhost:11434`）|
| `lmstudio` | 本地 LM Studio（默认 `localhost:1234`）|

用户在 `config.toml` 的 `[model_providers.<id>]` 段落里追加新的 provider，由 `merge_configured_model_providers`（`lib.rs:442`）合并。

> **当前 wire 协议只接受 `wire_api = "responses"`**。`chat` 已在上游废弃（`CHAT_WIRE_API_REMOVED_ERROR`，`lib.rs:45`）。

---

## 2. 四条接入路径

| # | 场景 | 改代码？ |
|---|---|---|
| **1** | 目标服务暴露 OpenAI Responses API 兼容端点（Azure / OpenRouter / LiteLLM / 自建网关 …）| 否 |
| **2** | 本地跑 Ollama / LM Studio | 否（直接用内置 provider）|
| **3** | 给模型增加工具能力 | 否（用 `codex mcp`，**不是** model provider）|
| **4** | 服务既非 Responses 兼容、也不是 Ollama/LMStudio | 是 |

下面分别展开。

---

## 3. 路径 1：`config.toml` 注入 provider

最常用。`~/.codex/config.toml`：

### 最小示例

```toml
model = "your-model-id"
model_provider = "corp"

[model_providers.corp]
name      = "Corp Gateway"
base_url  = "https://gateway.corp.example.com/v1"
env_key   = "CORP_TOKEN"
env_key_instructions = "从 https://corp.example.com/tokens 申请并 export CORP_TOKEN=..."
wire_api  = "responses"
```

跑：

```bash
export CORP_TOKEN=...
codex
```

### `ModelProviderInfo` 全字段

来源：`codex-rs/model-provider-info/src/lib.rs:84`。

| 字段 | 类型 | 作用 |
|---|---|---|
| `name` | String | 展示名 |
| `base_url` | String | 端点。省略时默认 `https://api.openai.com/v1`；ChatGPT 鉴权下默认 `https://chatgpt.com/backend-api/codex` |
| `env_key` | String | 从该环境变量读 Bearer token |
| `env_key_instructions` | String | 引导用户的提示文本 |
| `experimental_bearer_token` | String | 直接硬编码 token（仅供编程使用，**生产别用**）|
| `auth` | `ModelProviderAuthInfo` | 命令式鉴权（见下）|
| `aws` | `ModelProviderAwsAuthInfo` | SigV4，仅 `amazon-bedrock` 可用 |
| `wire_api` | `"responses"` | 当前唯一合法值 |
| `query_params` | `Map<str,str>` | 追加到 URL 的查询串 |
| `http_headers` | `Map<str,str>` | 固定 HTTP 头 |
| `env_http_headers` | `Map<header, env_var>` | 从环境变量取头，env 没设/为空就不发该头 |
| `request_max_retries` | u64 | 请求重试上限 |
| `stream_max_retries` | u64 | 流式断开重连上限 |
| `stream_idle_timeout_ms` | u64 | 流空闲超时（判断连接是否断了）|
| `websocket_connect_timeout_ms` | u64 | WebSocket 连接超时 |
| `requires_openai_auth` | bool | 是否跑 OpenAI 登录流程（普通第三方 = `false`，默认）|
| `supports_websockets` | bool | 是否启用 Responses-over-WebSocket |

### 命令式鉴权（`auth`）

定义在 `codex-rs/protocol/src/config_types.rs:472`。用于 token 需要动态生成 / 定期刷新（OIDC、企业 SSO 网关、`gcloud auth print-access-token` 之类）：

```toml
[model_providers.corp.auth]
command              = "corp-token"       # bare 名走 $PATH；带路径相对 cwd
args                 = ["--scope", "codex"]
timeout_ms           = 5000               # 单次命令超时
refresh_interval_ms  = 300000             # 0 = 仅在收到 401 时重跑
# cwd 默认 $CODEX_HOME，可显式指定
```

`auth` 与 `env_key` / `experimental_bearer_token` / `requires_openai_auth` **互斥**（`validate`，`lib.rs:149`）。

### 头部按环境变量注入

特别适合多租户：

```toml
[model_providers.corp.http_headers]
"X-App" = "codex"

[model_providers.corp.env_http_headers]
"X-Tenant" = "CORP_TENANT_ID"   # CORP_TENANT_ID 不存在/为空就不发这个头
```

### 选用 provider 的优先级

1. `--model-provider <id>`（命令行）
2. `-c model_provider=<id>`
3. `config.toml` 顶层 `model_provider = "<id>"`
4. 默认 `openai`

模型名同理：`--model` / `-c model=...` / 顶层 `model = "..."`。

---

## 4. 路径 2：本地 Ollama / LM Studio

两者均已内置注册，端口与协议见 `lib.rs:402`：

```toml
model          = "qwen2.5-coder:7b"
model_provider = "ollama"     # 或 "lmstudio"
```

启动时 Codex 通过 `codex-utils-oss::ensure_oss_provider_ready`（`codex-rs/utils/oss/src/lib.rs:17`）检查服务并按需拉模型（`codex-rs/ollama/src/pull.rs`）。

非默认端口/远端：复制一份当新 provider：

```toml
[model_providers.ollama-remote]
name     = "Remote Ollama"
base_url = "http://10.0.0.5:11434/v1"
wire_api = "responses"
```

> 历史上有过的 `ollama-chat` provider 已移除（`OLLAMA_CHAT_PROVIDER_REMOVED_ERROR`，`lib.rs:47`），统一用 `ollama` 即可。

---

## 5. 路径 3：MCP 工具不是 model provider

`codex mcp` 子命令挂外部 MCP server，是给主模型 **多一组工具调用**，与"换模型"是两件事。需要时见 `codex-rs/codex-mcp/src/mcp_connection_manager.rs` 与 `docs/getting-started.md` / `agents_md.md`。

---

## 6. 路径 4：写 Rust 新增 provider 类型

只有当 **目标服务既不是 Responses 兼容、也不是 Ollama/LMStudio 形态** 时才需要。当前仓内三个参考实现：

| 参考点 | 干嘛的 |
|---|---|
| `codex-rs/ollama/`、`codex-rs/lmstudio/` | 本地服务 + `ensure_oss_ready` + 模型 pull 的完整范式 |
| `codex-rs/model-provider/src/amazon_bedrock/` | 非 Bearer 鉴权（SigV4）怎么织入 `ApiProvider` |
| `codex-rs/model-provider-info/src/lib.rs:409` `built_in_model_providers` | 注册入口 |
| `codex-rs/model-provider/src/provider.rs` | 把 `ModelProviderInfo` 转成实际 `ApiProvider` / HTTP 客户端 |
| `codex-rs/protocol/src/openai_models.rs` | wire 类型 |

新增 wire 协议会同时碰 `codex-protocol` / `codex-api` / `codex-model-provider`；当前唯一在册的 `WireApi::Responses` 表明上游正主动收口到 Responses API，**建议优先考虑路径 1**（让目标服务前置一个 Responses 兼容网关，如 LiteLLM）。

---

## 7. 校验规则

来自 `ModelProviderInfo::validate`（`lib.rs:149`）与 `merge_configured_model_providers`：

- `auth.command` 不能为空字符串；
- `auth` 与 `env_key` / `experimental_bearer_token` / `requires_openai_auth` 互斥；
- `aws` 只对 `amazon-bedrock` 生效（`[model_providers.custom.aws]` 会被拒绝）；
- `aws` 与 `env_key` / `experimental_bearer_token` / `auth` / `requires_openai_auth` 互斥；
- `aws` 与 `supports_websockets` 互斥（SigV4 over WS 尚未实现）；
- 内置 provider **整体不可覆盖**；唯一例外：`amazon-bedrock` 允许只覆盖 `aws.profile` / `aws.region`。

任一违反 → 启动时报错并退出。

---

## 8. 实操速查

最常见的接入路径：

1. 让目标服务对外暴露 OpenAI Responses 兼容端点（自建走 [LiteLLM](https://github.com/BerriAI/litellm) 或类似网关最省事）；
2. `config.toml` 写一段 `[model_providers.<id>]`；
3. `export <ENV_KEY>=...`；
4. 启动：`codex -c model_provider=<id> -c model=<model-id>` 或把它们设为 `config.toml` 默认值。

调试技巧：

- `codex doctor` —— 检查配置、登录态、运行时健康；
- `codex debug models` —— dump 当前可见的模型目录（来自 `codex-models-manager`）；
- `codex debug models --bundled` —— 只看捆绑的目录，跳过远端刷新；
- `RUST_LOG=codex_model_provider=debug,codex_api=debug codex ...` —— 看请求/重试细节。

---

## 9. 关键文件索引

| 文件 | 内容 |
|---|---|
| `codex-rs/model-provider-info/src/lib.rs` | `ModelProviderInfo`、内置 provider、合并/校验 |
| `codex-rs/protocol/src/config_types.rs` | `ModelProviderAuthInfo`、`ModelProviderAwsAuthInfo` |
| `codex-rs/model-provider/src/provider.rs` | provider → HTTP 客户端的运行时层 |
| `codex-rs/model-provider/src/amazon_bedrock/` | Bedrock SigV4 实现 |
| `codex-rs/ollama/` `codex-rs/lmstudio/` | 本地服务参考实现 |
| `codex-rs/utils/oss/src/lib.rs` | Ollama/LMStudio 启动前的准备逻辑 |
| `codex-rs/core/src/config/` | `model_provider` / `model_providers` 的解析入口 |
| `codex-rs/core/config.schema.json` | 配置 schema（修改 `ConfigToml` 后 `just write-config-schema` 重生成）|

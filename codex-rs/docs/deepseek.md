# 接入 DeepSeek（路径 A：sidecar 方案）

> 本文是 DeepSeek 集成路线的第一步：**不改 Codex 任何代码**，用一个本地翻译层把 Codex 的 OpenAI Responses API 请求转发到 DeepSeek。
>
> 后续会有路径 B（仓内 Rust `codex-deepseek-proxy` 翻译 crate）和路径 C（注册成内置 provider 自动拉起），届时本文档将标注为"过渡方案"。
>
> 相关参考：DeepSeek 官方集成指南 <https://github.com/deepseek-ai/awesome-deepseek-agent/blob/main/docs/codex.md>，本仓库 `codex-rs/docs/model-providers.md`。

---

## 1. 为什么需要 sidecar

| 事实 | 影响 |
|---|---|
| Codex 当前只接受 `wire_api = "responses"`（OpenAI Responses API），`chat` 已被上游移除 | Codex 端无法直连任何 Chat Completions 服务 |
| DeepSeek 暴露 `https://api.deepseek.com/v1/chat/completions`（OpenAI 兼容）与 `https://api.deepseek.com/anthropic`（Anthropic 兼容），**不直接提供 Responses API** | DeepSeek 端不能被 Codex 直连 |

→ 二者之间必须有一层 **`/v1/responses` ↔ DeepSeek 的翻译进程**。本文用的就是这层 sidecar。

```
┌─────────┐  /v1/responses  ┌────────────┐  /v1/chat/completions  ┌──────────────┐
│  codex  │ ──────────────▶ │ sidecar    │ ─────────────────────▶ │ api.deepseek │
│ (rust)  │ ◀── SSE 流 ──── │ (translate) │ ◀──── SSE 流 ───────── │    .com      │
└─────────┘                 └────────────┘                        └──────────────┘
   127.0.0.1                   127.0.0.1                                 ↑
   (本地进程)                 :38440 / :4000                    DEEPSEEK_API_KEY
```

---

## 2. 选 sidecar

下面列两种已经验证或被 DeepSeek 官方推荐的 sidecar。**Codex 端配置完全一样**，区别只在 sidecar 内部走哪条 DeepSeek 上游通道与运行时栈。

| Sidecar | 上游通道 | 运行时 | 备注 |
|---|---|---|---|
| **`codex deepseek-proxy`** | DeepSeek `/v1/chat/completions` | 无（内置 Rust）| 跟 codex 同一个二进制；单进程；已实现 reasoning / 工具调用翻译 |
| **Moon Bridge** | DeepSeek `/anthropic` | Go 1.25+ | DeepSeek 官方文档推荐；插件丰富 |
| **LiteLLM Proxy** | DeepSeek `/v1/chat/completions` | Python 3.10+ | 通用网关，可同时挂多家 |

任选其一。**推荐 `codex deepseek-proxy`**，省一个进程、零额外依赖。

---

## 3. 选项 1（推荐）：内置 + 自动拉起

仓内的 `codex-deepseek-proxy` crate 直接被链接进 codex 主二进制。当 `model_provider = "deepseek"` 且 `base_url` 指向 loopback 时，**codex 启动会在同一进程里自动 spawn 一个 axum 监听任务**，对外讲 Responses API、对内打 DeepSeek `/v1/chat/completions`。退出时随 codex 一起结束，零生命周期管理。

#### 最小可用配置

`~/.codex/config.toml`（顶层）：

```toml
model              = "deepseek-v4-pro"      # 或 deepseek-v4-flash
model_provider     = "deepseek"
model_catalog_json = "~/.codex/models_catalog.json"
```

注意 **不再需要写 `[model_providers.deepseek]`**。内置默认：
- `base_url = "http://127.0.0.1:38440/v1"`
- `env_key  = "DEEPSEEK_API_KEY"`
- `wire_api = "responses"`

```bash
export DEEPSEEK_API_KEY=sk-...
codex
```

启动时 stderr 会有一条 `INFO auto-spawned codex-deepseek-proxy addr=127.0.0.1:38440`。

#### 想覆盖默认？写 `[model_providers.deepseek]`

任何字段都可以替换内置默认，整个 section 完整生效（不只是合并），常见三种：

```toml
# 1) 用 keychain / 1Password 等命令拿 token（不走环境变量）
[model_providers.deepseek]
base_url = "http://127.0.0.1:38440/v1"
wire_api = "responses"

[model_providers.deepseek.auth]
command = "security"
args    = ["find-generic-password", "-a", "codex", "-s", "codex-deepseek-key", "-w"]
timeout_ms          = 3000
refresh_interval_ms = 0
```

```toml
# 2) 换端口（避免冲突）
[model_providers.deepseek]
base_url = "http://127.0.0.1:39000/v1"
env_key  = "DEEPSEEK_API_KEY"
wire_api = "responses"
```

```toml
# 3) 指向外部 sidecar（关闭 auto-spawn）
[model_providers.deepseek]
base_url = "http://my-gateway.corp.example.com/v1"   # 非 loopback → 跳过 auto-spawn
env_key  = "DEEPSEEK_API_KEY"
wire_api = "responses"
```

**触发 auto-spawn 的条件**（满足才会拉起）：

| 条件 | 命中 |
|---|---|
| `model_provider == "deepseek"` | ✅ |
| `base_url` scheme = `http` 且 host ∈ {`127.0.0.1`, `localhost`, `::1`} 且有显式端口 | ✅ |
| 该端口未被占用 | ✅（占用则假定已有 proxy 在跑、跳过自动 spawn）|

否则 codex 不动这个 base_url，按普通 provider 处理。

**支持矩阵**：

- ✅ 文本流式（B2）
- ✅ Tool calling（B3）—— 并发 tool calls 自动按 `output_index` 分配
- ✅ Reasoning 流（B4）—— `reasoning_effort` 映射 + `delta.reasoning_content` → `response.reasoning_summary_*`
- ✅ `usage.reasoning_tokens` 透传
- ✅ Moon Bridge 对齐：`stream_options.include_usage`、thinking 模式清 sampling 参数、assistant tool_calls 强制带 `reasoning_content`
- ⏳ 多模态（图片输入）—— 未实现
- ⏳ 结构化输出（response_format=json_schema）—— 透传，DeepSeek 端是否生效取决于上游

#### 不想用 auto-spawn？

也可以手工独立运行 proxy（替代 codex 默认行为）：

```bash
codex deepseek-proxy --listen 127.0.0.1:38440 --print-listen
```

然后第二终端 `codex` 启动时端口已被占用 → auto-spawn 自动跳过、直接连你的实例。

**调试**：

```bash
RUST_LOG=codex_deepseek_proxy=debug codex
```

---

## 4. 选项 2：Moon Bridge（DeepSeek 官方路径）

> 跟随 DeepSeek 官方文档；摘录到这里方便对照。

```bash
git clone https://github.com/ZhiYi-R/moon-bridge.git
cd moon-bridge
```

写 `config.yml`：

```yaml
mode: "Transform"

server:
  addr: "127.0.0.1:38440"

models:
  deepseek-v4-pro:
    context_window: 1000000
    max_output_tokens: 384000
    default_reasoning_level: "high"
    supported_reasoning_levels:
      - effort: "high"
      - effort: "xhigh"
    supports_reasoning_summaries: true
    default_reasoning_summary: "auto"
    extensions:
      deepseek_v4:
        enabled: true

providers:
  deepseek:
    base_url: "https://api.deepseek.com/anthropic"
    api_key: "sk-your-deepseek-api-key"     # ← 填这里
    offers:
      - model: deepseek-v4-pro

routes:
  moonbridge:
    model: deepseek-v4-pro
    provider: deepseek

defaults:
  model: moonbridge
  max_tokens: 65536
```

启动：

```bash
go run ./cmd/moonbridge --config config.yml
# Listening on 127.0.0.1:38440
# 端点：http://127.0.0.1:38440/v1/responses
```

保持这个终端开着。

> Moon Bridge 还自带 `--print-codex-config` 选项可以**直接生成** Codex 的 `config.toml`，参考它的官方文档。但本文 §5 给出手工配置，对调试更友好。

---

## 5. 选项 3：LiteLLM Proxy

LiteLLM 是通用的 LLM 网关。用 `pipx` / `uv` / `pip` 装：

```bash
# 推荐
pipx install 'litellm[proxy]'
# 或
uv tool install 'litellm[proxy]'
```

写 `litellm.yaml`：

```yaml
model_list:
  - model_name: deepseek-v4-flash
    litellm_params:
      model: deepseek/deepseek-v4-flash
      api_key: os.environ/DEEPSEEK_API_KEY
  - model_name: deepseek-v4-pro
    litellm_params:
      model: deepseek/deepseek-v4-pro
      api_key: os.environ/DEEPSEEK_API_KEY

general_settings:
  master_key: sk-local-anything    # 本地随便填，下面 Codex 端用同一个
```

启动：

```bash
export DEEPSEEK_API_KEY=sk-your-deepseek-key
litellm --config litellm.yaml --port 4000
# 端点：http://127.0.0.1:4000/v1/responses
```

> LiteLLM 近期加入了 `/v1/responses` 路由的翻译能力；用前请用 `litellm --version` 确认版本，并参考 LiteLLM 官方"Responses API"文档确认你装的版本支持。如果你的版本只支持 `/v1/chat/completions`，则需要换回 Moon Bridge。

---

## 6. Codex 端配置

不论选哪个 sidecar，Codex 端 `~/.codex/config.toml` 都长一样（只是端口和环境变量名换一下）：

### 6.1 内置 + auto-spawn（推荐）

> 详细说明见 §3。这里只放最简版回顾。

```bash
cp codex-rs/deepseek-proxy/examples/models_catalog.json \
   "${CODEX_HOME:-$HOME/.codex}/models_catalog.json"
```

`~/.codex/config.toml`：

```toml
model              = "deepseek-v4-pro"
model_provider     = "deepseek"
model_catalog_json = "~/.codex/models_catalog.json"
# [model_providers.deepseek] 可省略 —— 内置默认含 base_url / env_key / wire_api
```

```bash
export DEEPSEEK_API_KEY=sk-...
codex          # 单条命令，无需另起 proxy
```

> **TOML 提示**：顶层字段必须放在所有 `[section]` 之前；不要用 `echo >>` 追加。

### 6.2 Moon Bridge 版

```toml
model          = "moonbridge"           # ← 与 Moon Bridge 的 routes 名一致
model_provider = "deepseek"

[model_providers.deepseek]
name     = "DeepSeek (via Moon Bridge)"
base_url = "http://127.0.0.1:38440/v1"
wire_api = "responses"
# Moon Bridge 不需要再次鉴权，给个空 env key 占位即可
experimental_bearer_token = "unused"
```

### 6.3 LiteLLM 版

```toml
model          = "deepseek-v4-pro"      # 或 deepseek-v4-flash
model_provider = "deepseek"

[model_providers.deepseek]
name     = "DeepSeek (via LiteLLM)"
base_url = "http://127.0.0.1:4000/v1"
env_key  = "LITELLM_MASTER_KEY"
wire_api = "responses"
```

```bash
export LITELLM_MASTER_KEY=sk-local-anything   # 与 litellm.yaml 的 master_key 一致
```

### 6.4 配置字段速查

字段含义见 `codex-rs/docs/model-providers.md` §3。

---

## 7. 验证

启动新终端：

```bash
cd /path/to/your-project
codex
```

第一条消息发出后，sidecar 终端应该能看到 `POST /v1/responses` 的日志。

直接打 sidecar 看是否通：

```bash
curl http://127.0.0.1:38440/v1/responses \
  -H "Content-Type: application/json" \
  -d '{"model":"moonbridge","input":"hello","max_output_tokens":64}'
```

Codex 内置诊断：

```bash
codex doctor
RUST_LOG=codex_model_provider=debug,codex_api=debug codex
```

---

## 8. 故障排查

| 症状 | 排查 |
|---|---|
| `connection refused` | sidecar 没起来，或 `base_url` 端口不一致 |
| `401 / 403` | sidecar 端的 DeepSeek API key 错；或 LiteLLM 的 `LITELLM_MASTER_KEY` 与 `config.toml` 不匹配 |
| `402` | DeepSeek Platform 账户余额不足 |
| Codex 启动报 `provider ... unknown` | `[model_providers.deepseek]` 段没加，或 `model_provider` 拼错 |
| 流式响应卡住 | sidecar 不支持 SSE；切换 sidecar 或升级版本 |
| reasoning 字段丢失 | sidecar 未实现 `reasoning_content` → Responses reasoning summary 的映射；属于翻译质量问题（路径 B 的优化目标） |
| 工具调用结果异常 | sidecar 的 tool calling schema 映射有缺陷；同上 |

---

## 9. 已知局限

Responses API 有一些 chat completions 等不到等价物的特性，经过翻译层时可能降级：

- **server-side conversation state**（`previous_response_id`、`store`）：sidecar 通常无法持久化，Codex 端的 `compact` / `resume` 仍可用，但 token 计费走完整历史
- **结构化输出（response_format=json_schema）**：部分 sidecar 直通 DeepSeek，部分会丢失字段约束
- **reasoning summary**：v4-pro / v4-flash 在 thinking 模式下输出 `delta.reasoning_content`（请求侧 `reasoning_effort` 取 `high` 或 `max` 启用），各 sidecar 对其向 Responses `reasoning_summary` 的映射策略不同
- **多模态**：纯文本模型，发图片会被拒绝或忽略（取决于 sidecar）
- **超时与重试**：HTTP 层重试由 Codex 控制（`request_max_retries`、`stream_idle_timeout_ms`），但 sidecar 本身故障不会触发 provider 切换

---

## 10. 路线图

| 阶段 | 状态 | 说明 |
|---|---|---|
| **A** 外部 sidecar（Moon Bridge / LiteLLM） | ✅ 落地 | 仍保留作为备选 |
| **B** `codex-deepseek-proxy` crate | ✅ 落地 | B0 骨架 / B1 请求翻译 / B2 SSE 文本 / B3 tool calling / B4 reasoning / B5 polish |
| **C** 内置 provider + in-process auto-spawn | ✅ 落地 | `built_in_model_providers` 注册 `deepseek`；TUI/exec 启动路径调 `codex_deepseek_proxy::ensure_running`；proxy 跑在 codex 同进程的 tokio 任务里，端口冲突时自动跳过 |

---

## 11. 参考

- DeepSeek 官方集成指南：<https://github.com/deepseek-ai/awesome-deepseek-agent/blob/main/docs/codex.md>
- Moon Bridge：<https://github.com/ZhiYi-R/moon-bridge>
- LiteLLM Proxy：<https://docs.litellm.ai/docs/proxy>
- 本仓库：`codex-rs/docs/model-providers.md` —— 完整的 `ModelProviderInfo` 字段表与校验规则
- 本仓库：`codex-rs/model-provider-info/src/lib.rs` —— provider 注册与合并逻辑

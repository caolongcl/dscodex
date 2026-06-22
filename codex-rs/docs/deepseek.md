# 接入 DeepSeek

> Codex 通过**内置的进程内翻译 transport** 直连 DeepSeek —— 不需要任何外部 proxy 进程,也不监听任何本地端口。早期方案(外部 sidecar、daemon 内嵌 proxy、loopback 端口 + auto-spawn)均已被取代;外部 proxy 仍可作为备选,见 §5。
>
> 参考:DeepSeek 官方集成指南 <https://github.com/deepseek-ai/awesome-deepseek-agent/blob/main/docs/codex.md>;本仓库 `codex-rs/docs/model-providers.md`。

---

## 1. 为什么需要翻译

| 事实 | 影响 |
|---|---|
| Codex 只接受 `wire_api = "responses"`(OpenAI Responses API),`chat` 已被上游移除 | Codex 端不能直连 Chat Completions 服务 |
| DeepSeek 暴露 `https://api.deepseek.com/v1/chat/completions`(OpenAI 兼容),**不提供 Responses API** | DeepSeek 端不能被 Codex 直连 |

→ 二者之间必须有一层 `/responses` ↔ `/chat/completions` 翻译。Codex 现在把它做在**自己进程里**(`codex-deepseek-proxy` crate 提供的 `DeepSeekTransport`),而不是一个独立 HTTP 进程。

```
┌─────────┐  Responses 请求   ┌──────────────────────┐  /v1/chat/completions  ┌──────────────┐
│  codex  │ ─(进程内,无端口)─▶│ DeepSeekTransport    │ ─────────────────────▶ │ api.deepseek │
│ (rust)  │ ◀─ Responses SSE ─│ (HttpTransport,翻译) │ ◀──── Chat SSE ─────────│    .com      │
└─────────┘                   └──────────────────────┘                        └──────────────┘
                                                                                 DEEPSEEK_API_KEY
```

`core/src/client.rs` 在 provider 的 `base_url` host 为 `api.deepseek.com` 时自动选用该 transport;否则走普通 reqwest transport(见 §5)。Authorization 由 codex 按 `env_key` 注入,transport 原样转发。

---

## 2. 设置(推荐:内置)

把模型目录放到 `CODEX_HOME`:

```bash
cp codex-rs/deepseek-proxy/examples/models_catalog.json \
   "${CODEX_HOME:-$HOME/.codex}/models_catalog.json"
```

`~/.codex/config.toml`(顶层字段须放在所有 `[section]` 之前;别用 `echo >>` 追加):

```toml
model              = "deepseek-v4-pro"      # 或 deepseek-v4-flash
model_provider     = "deepseek"
model_catalog_json = "~/.codex/models_catalog.json"
# [model_providers.deepseek] 可省略 —— 内置默认:
#   base_url = "https://api.deepseek.com/v1", env_key = "DEEPSEEK_API_KEY", wire_api = "responses"
```

```bash
export DEEPSEEK_API_KEY=sk-...
codex          # 单条命令,无需另起 proxy,不监听端口
```

#### 用命令(keychain / 1Password)取 token

```toml
[model_providers.deepseek]
base_url = "https://api.deepseek.com/v1"
wire_api = "responses"

[model_providers.deepseek.auth]
command = "security"
args    = ["find-generic-password", "-a", "codex", "-s", "codex-deepseek-key", "-w"]
timeout_ms          = 3000
refresh_interval_ms = 0
```

> 写了 `[model_providers.deepseek]` 会**完整替换**内置默认(不是合并)。只要 `base_url` 仍是 `https://api.deepseek.com`,就继续走进程内翻译 transport。

---

## 3. 支持矩阵

- ✅ 文本流式
- ✅ Tool calling —— 并发 tool calls 按 `output_index` 分配
- ✅ MCP / 命名空间工具 —— codex 的 `type:"namespace"` 工具摊平成 `<ns>__<tool>` function,回程按映射重建 namespace
- ✅ Reasoning 流 —— `reasoning_effort` 映射 + `delta.reasoning_content` → `response.reasoning_summary_*`;思考"关"哨兵 effort `"none"/"off"` → `thinking:{type:disabled}`(DeepSeek V4 思考默认开)
- ✅ `usage.reasoning_tokens` 透传
- ⏳ 多模态(图片输入)、结构化输出(json_schema)—— 未实现 / 透传

---

## 4. 验证 / 调试

```bash
codex
codex doctor
RUST_LOG=codex_deepseek_proxy=debug,codex_api=debug codex
```

进程内 transport **不监听端口**,所以不会有 `127.0.0.1:38440` 之类的本地监听(`lsof -i :38440` 应为空)。

---

## 5. 备选:外部 Responses proxy

把 `base_url` 指向一个**自己讲 Responses API** 的外部 proxy(Moon Bridge、LiteLLM …)。此时 host 非 `api.deepseek.com`,codex 用普通 reqwest transport 直连它、**不做**进程内翻译(翻译交给该 proxy)。

### Moon Bridge(DeepSeek 官方路径,走 `/anthropic`)

```bash
git clone https://github.com/ZhiYi-R/moon-bridge.git && cd moon-bridge
```
`config.yml`(关键段):
```yaml
server: { addr: "127.0.0.1:38440" }
providers:
  deepseek:
    base_url: "https://api.deepseek.com/anthropic"
    api_key: "sk-your-deepseek-api-key"
    offers: [{ model: deepseek-v4-pro }]
routes: { moonbridge: { model: deepseek-v4-pro, provider: deepseek } }
defaults: { model: moonbridge, max_tokens: 65536 }
```
```bash
go run ./cmd/moonbridge --config config.yml   # 127.0.0.1:38440
```
Codex 端:
```toml
model          = "moonbridge"
model_provider = "deepseek"
[model_providers.deepseek]
base_url = "http://127.0.0.1:38440/v1"
wire_api = "responses"
experimental_bearer_token = "unused"
```

### LiteLLM
```bash
pipx install 'litellm[proxy]'
```
`litellm.yaml`:
```yaml
model_list:
  - { model_name: deepseek-v4-pro, litellm_params: { model: deepseek/deepseek-v4-pro, api_key: os.environ/DEEPSEEK_API_KEY } }
general_settings: { master_key: sk-local-anything }
```
```bash
export DEEPSEEK_API_KEY=sk-...
litellm --config litellm.yaml --port 4000
```
Codex 端 `base_url = "http://127.0.0.1:4000/v1"`、`env_key = "LITELLM_MASTER_KEY"`。
> LiteLLM 需版本支持 `/v1/responses` 路由(`litellm --version` 确认)。

---

## 6. 故障排查

| 症状 | 排查 |
|---|---|
| `connection refused` / 网络错 | 能否直连 `api.deepseek.com`?transport 用 `no_proxy` 直连,绕开本机 HTTP(S)_PROXY |
| `401 / 403` | `DEEPSEEK_API_KEY` 错或未设(外部 proxy 模式则查它的 key) |
| `402` | DeepSeek 账户余额不足 |
| `provider ... unknown` | `model_provider` 拼错 |
| reasoning / 工具结果异常 | 翻译质量问题,见 `deepseek-proxy` crate `translate.rs` / `stream.rs` |

---

## 7. 已知局限

- server-side conversation state(`previous_response_id`、`store`):无持久化;`compact`/`resume` 仍可用但 token 计费走完整历史。
- 结构化输出(json_schema):透传,DeepSeek 端是否生效取决于上游。
- reasoning summary:thinking 模式(`reasoning_effort` = `high`/`max`)下 `delta.reasoning_content` → Responses `reasoning_summary`。
- 多模态:纯文本模型,图片被拒。
- 重试:HTTP 层重试由 Codex 控制(`request_max_retries`、`stream_idle_timeout_ms`);上游故障不触发 provider 切换。

---

## 8. 演进

| 阶段 | 状态 | 说明 |
|---|---|---|
| A 外部 sidecar(Moon Bridge / LiteLLM) | ✅ 保留为备选(§5) | 不改 codex 代码 |
| B 仓内 `codex-deepseek-proxy` crate(HTTP server)+ in-process auto-spawn | ⛔ 已取代 | 曾在 loopback 端口跑 axum server,启动时 `ensure_running` 自动拉起 |
| C 进程内 `DeepSeekTransport`(无 proxy / 无端口) | ✅ 现状 | crate 改为提供 `HttpTransport`;`core/src/client.rs` 用 enum dispatch 选用;删 axum server、`codex deepseek-proxy` 子命令、auto-spawn 钩子、loopback 端口 |

---

## 9. 参考

- DeepSeek 官方集成指南:<https://github.com/deepseek-ai/awesome-deepseek-agent/blob/main/docs/codex.md>
- Moon Bridge:<https://github.com/ZhiYi-R/moon-bridge>
- LiteLLM Proxy:<https://docs.litellm.ai/docs/proxy>
- `codex-rs/docs/model-providers.md` —— `ModelProviderInfo` 字段表与校验规则
- `codex-rs/deepseek-proxy/` —— 纯翻译(`translate.rs`/`stream.rs`)+ `DeepSeekTransport`(`transport.rs`)
- `codex-rs/model-provider-info/src/lib.rs` —— provider 注册与合并逻辑

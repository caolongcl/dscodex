# Moon Bridge sidecar（Codex × DeepSeek）

把 Moon Bridge 作为本地协议转换层，让 Codex 通过 OpenAI Responses API 调用 DeepSeek。详细背景见 [`codex-rs/docs/deepseek.md`](../../codex-rs/docs/deepseek.md)。

## 目录布局

| 路径 | 内容 | 入库？ |
|---|---|---|
| `setup.sh` | 克隆/更新 `upstream/`，从 example 引导出本地 `config.yml`，校验 Go 版本 | ✅ |
| `run.sh` | 用 `config.yml` 起 Moon Bridge（监听 `127.0.0.1:38440`）| ✅ |
| `gen-codex-config.sh` | 调 `moonbridge --print-codex-config` 把 `config.toml` + `models_catalog.json` 写到 `$CODEX_HOME` | ✅ |
| `config.example.yml` | Codex × DeepSeek 配置模板（含 API key 占位符）| ✅ |
| `config.yml` | 你的本地副本（**含真实 API key，已 gitignore**）| ❌ |
| `upstream/` | `git clone github.com/ZhiYi-R/moon-bridge`（gitignore，由 `setup.sh` 管理）| ❌ |

## 快速开始

```bash
# 1. 准备依赖（克隆/更新 upstream、生成 config.yml）
./setup.sh

# 2. 编辑 config.yml，把 sk-REPLACE-WITH-YOUR-DEEPSEEK-API-KEY 换成真实 key
#    申请地址：https://platform.deepseek.com/api_keys
$EDITOR config.yml

# 3a. 让 Moon Bridge 自动写 Codex 配置（推荐）
./gen-codex-config.sh

#    或 3b. 自己写 Codex 配置：参考 codex-rs/docs/deepseek.md §5.1

# 4. 在另一个终端启动 Moon Bridge
./run.sh
# 监听 http://127.0.0.1:38440/v1/responses

# 5. 在第三个终端跑 Codex
cd /path/to/your-project
codex
```

## Go 版本

Moon Bridge 的 `go.mod` 钉死 `go 1.25.0`。本地装的是更低版本时，`setup.sh` 会给警告，`run.sh` / `gen-codex-config.sh` 通过 `GOTOOLCHAIN=auto`（Go 1.21+ 默认）让 Go 自动下载并使用 1.25 工具链 —— 第一次运行会稍慢。如果你的环境完全禁网或卡在工具链下载，直接装 Go 1.25+：<https://go.dev/dl/>。

## 验证

```bash
# 直打 sidecar
curl http://127.0.0.1:38440/v1/responses \
  -H "Content-Type: application/json" \
  -d '{"model":"moonbridge","input":"hello","max_output_tokens":64}'

# Codex 端
codex doctor
RUST_LOG=codex_model_provider=debug,codex_api=debug codex
```

更多故障排查见 [`codex-rs/docs/deepseek.md` §7](../../codex-rs/docs/deepseek.md)。

## 更新

```bash
./setup.sh    # 拉 upstream 最新 main，保留 config.yml
```

## 卸载

```bash
rm -rf upstream config.yml
# 如果之前跑过 gen-codex-config.sh，记得还原 $CODEX_HOME/config.toml.bak.*
```

<p align="center"><strong>Codex CLI</strong> is a coding agent from OpenAI that runs locally on your computer.</p>
<p align="center">
  <img src="https://github.com/openai/codex/blob/main/.github/codex-cli-splash.png" alt="Codex CLI splash" width="80%" />
</p>
</br>
If you want Codex in your code editor (VS Code, Cursor, Windsurf), <a href="https://developers.openai.com/codex/ide">install in your IDE.</a>
</br>If you want the desktop app experience, run <code>codex app</code> or visit <a href="https://chatgpt.com/codex?app-landing-page=true">the Codex App page</a>.
</br>If you are looking for the <em>cloud-based agent</em> from OpenAI, <strong>Codex Web</strong>, go to <a href="https://chatgpt.com/codex">chatgpt.com/codex</a>.</p>

---

> **This fork** adds native DeepSeek integration and third-party model provider support to Codex CLI.
> Use DeepSeek V4, Moon Bridge, LiteLLM, Ollama, LM Studio, or any OpenAI Responses API-compatible endpoint.

---

## Quickstart

### Installing and running Codex CLI

Run the following on Mac or Linux to install Codex CLI:

```shell
curl -fsSL https://chatgpt.com/codex/install.sh | sh
```

Run the following on Windows to install Codex CLI:

```
powershell -ExecutionPolicy ByPass -c "irm https://chatgpt.com/codex/install.ps1 | iex"
```

Codex CLI can also be installed via the following package managers:

```shell
# Install using npm
npm install -g @openai/codex
```

```shell
# Install using Homebrew
brew install --cask codex
```

Then simply run `codex` to get started.

<details>
<summary>You can also go to the <a href="https://github.com/openai/codex/releases/latest">latest GitHub Release</a> and download the appropriate binary for your platform.</summary>

Each GitHub Release contains many executables, but in practice, you likely want one of these:

- macOS
  - Apple Silicon/arm64: `codex-aarch64-apple-darwin.tar.gz`
  - x86_64 (older Mac hardware): `codex-x86_64-apple-darwin.tar.gz`
- Linux
  - x86_64: `codex-x86_64-unknown-linux-musl.tar.gz`
  - arm64: `codex-aarch64-unknown-linux-musl.tar.gz`

Each archive contains a single entry with the platform baked into the name (e.g., `codex-x86_64-unknown-linux-musl`), so you likely want to rename it to `codex` after extracting it.

</details>

### Using Codex with your ChatGPT plan

Run `codex` and select **Sign in with ChatGPT**. We recommend signing into your ChatGPT account to use Codex as part of your Plus, Pro, Business, Edu, or Enterprise plan. [Learn more about what's included in your ChatGPT plan](https://help.openai.com/en/articles/11369540-codex-in-chatgpt).

You can also use Codex with an API key, but this requires [additional setup](https://developers.openai.com/codex/auth#sign-in-with-an-api-key).

---

## Roles

This fork adds a role system: switch Codex from a coding agent to a writer, researcher, or any persona you define — while keeping its tools, sandboxing, and approval rules intact.

```
/role            # pick writer / researcher / default, or your own
```

Add custom roles as markdown files under `~/.codex/roles/<name>.md`, set a default with `role = "writer"` in `config.toml`, or bundle a role with a model in a profile. See the [roles guide](docs/roles.md).

---

## DeepSeek Integration

This fork ships a built-in `codex-deepseek-proxy` that translates OpenAI Responses API ↔ DeepSeek Chat Completions in-process — zero extra dependencies, no sidecar process to manage.

### Quick start (auto-spawn)

1. **Get a DeepSeek API key** from [platform.deepseek.com](https://platform.deepseek.com).

2. **Write config** (`~/.codex/config.toml`):

```toml
model              = "deepseek-v4-pro"      # or deepseek-v4-flash
model_provider     = "deepseek"
model_catalog_json = "~/.codex/models_catalog.json"
```

3. **Copy the models catalog** (from this repo):

```bash
cp codex-rs/deepseek-proxy/examples/models_catalog.json \
   ~/.codex/models_catalog.json
```

4. **Run:**

```bash
export DEEPSEEK_API_KEY=sk-...
codex
```

Codex auto-spawns the proxy on `127.0.0.1:38440`. Look for `INFO auto-spawned codex-deepseek-proxy` on stderr.

### Supported features

| Feature | Status |
|---|---|
| Text streaming (SSE) | ✅ |
| Tool calling (concurrent) | ✅ |
| Reasoning (thinking) stream | ✅ |
| Usage / token counting | ✅ |
| Multi-modal (image input) | ⏳ |
| Structured output (json_schema) | ⏳ |

### Alternative sidecars

You can also use **Moon Bridge** (DeepSeek official) or **LiteLLM Proxy** if you prefer an external gateway. See [`codex-rs/docs/deepseek.md`](codex-rs/docs/deepseek.md) for full instructions.

### Other model providers

Configure any OpenAI Responses API-compatible endpoint in `~/.codex/config.toml`:

```toml
model = "your-model-id"
model_provider = "my-provider"

[model_providers.my-provider]
name     = "My Provider"
base_url = "https://your-gateway.example.com/v1"
env_key  = "MY_API_KEY"
wire_api = "responses"
```

Built-in providers: **OpenAI**, **Ollama** (`localhost:11434`), **LM Studio** (`localhost:1234`).  
See [`codex-rs/docs/model-providers.md`](codex-rs/docs/model-providers.md) for the full reference.

---

## Build from source

```bash
git clone https://github.com/caolongcl/dscodex.git
cd dscodex

# Quick install (debug build → ~/.local/bin/codex)
scripts/build-and-install-local.sh

# Release build
scripts/build-and-install-local.sh --release
```

See [`docs/install.md`](docs/install.md) for detailed build prerequisites and platform-specific notes.

---

## Docs

- [**Codex Documentation**](https://developers.openai.com/codex)
- [**Roles guide**](docs/roles.md)
- [**DeepSeek integration guide**](codex-rs/docs/deepseek.md)
- [**Third-party model providers**](codex-rs/docs/model-providers.md)
- [**Contributing**](./docs/contributing.md)
- [**Installing & building**](./docs/install.md)
- [**Open source fund**](./docs/open-source-fund.md)

This repository is licensed under the [Apache-2.0 License](LICENSE).

# Example assets for `codex deepseek-proxy`

## `models_catalog.json`

Codex's `/model` picker and per-model behaviour (reasoning UI, context window
budgeting, apply_patch tool wiring …) come from a model catalog. By default it
uses the OpenAI catalog bundled with codex; when you run DeepSeek you want
codex to know about `deepseek-v4-pro` / `deepseek-v4-flash` instead, otherwise
you'll see `Model metadata for deepseek-v4-pro not found` warnings and the
picker keeps showing GPT-5.x.

This file is a minimal valid catalog that **replaces** the bundled one for the
current process (per the `model_catalog_json` semantics).

### Install

```bash
cp codex-rs/deepseek-proxy/examples/models_catalog.json "${CODEX_HOME:-$HOME/.codex}/models_catalog.json"
```

Then in `~/.codex/config.toml`:

```toml
model              = "deepseek-v4-pro"
model_provider     = "deepseek"
model_catalog_json = "~/.codex/models_catalog.json"
```

Restart `codex` (the in-tree proxy auto-spawns on first use; see
`codex-rs/docs/deepseek.md` §3). `/model` should now show:

```
1. deepseek-v4-pro    (default)
2. deepseek-v4-flash
```

### Tunables

| Field | Current | Notes |
|---|---|---|
| `context_window` | 128000 | Conservative; bump if your account's tier allows more |
| `effective_context_window_percent` | 80 | Codex reserves 20% for system prompts + tool overhead |
| `truncation_policy.limit` | 10000 | Per-message tool-output truncation (tokens) |
| `apply_patch_tool_type` | `"freeform"` | DeepSeek follows OpenAI Chat Completions; freeform apply_patch works |
| `supported_reasoning_levels` | `[high, xhigh]` | UI labels; `xhigh` is mapped to DeepSeek's `max` by the proxy |
| `web_search_tool_type` | `"text"` | Cosmetic; only used when `supports_search_tool` is true. DeepSeek doesn't expose web search, so this never fires |
| `input_modalities` | `["text"]` | DeepSeek V4 is text-only via `/chat/completions` |

If you decide later to bring the GPT models back alongside DeepSeek, paste
their entries from `codex-rs/models-manager/models.json` into the `models`
array. Codex picks the active model by `slug`, so coexistence is fine.

# Roles

> Fork feature: not part of upstream Codex.

By default Codex is a coding agent. A **role** reframes its persona — writer, researcher, or anything you define — without weakening the harness: tool usage, sandboxing, and approval rules from the base instructions stay in force. The role text is injected as a dedicated `<role_spec>` developer message on top of the base instructions instead of replacing them, so it works with any model provider (including DeepSeek and other non-OpenAI models configured in this fork).

## Quick start

Switch roles inside the TUI:

```
/role
```

Pick a role from the list. The choice applies to the next turn of the current conversation and is saved to `config.toml` as the default for new sessions. Pick `default` to return to the stock coding agent.

## Built-in roles

| Name         | Persona                                                            |
| ------------ | ------------------------------------------------------------------ |
| `default`    | Stock coding agent — no extra role instructions                    |
| `writer`     | Literary writing partner for poetry, fiction, essays, and articles |
| `researcher` | Research collaborator for scientific and mathematical work         |

## Custom roles

Drop a markdown file into `$CODEX_HOME/roles/` (usually `~/.codex/roles/`):

```
~/.codex/roles/
  tutor.md        # available as the role `tutor`
  poet.md         # available as the role `poet`
```

The file body is the role prompt. The first heading (or first non-empty line) becomes the description shown in the `/role` picker. A custom file with the same name as a built-in role (for example `writer.md`) replaces that preset; the reserved name `default` cannot be shadowed.

Example `~/.codex/roles/poet.md`:

```markdown
# Classical Chinese poet

You are a master of classical Chinese poetry. When given a theme, compose
regulated verse (律诗) or ci (词) honoring the requested tone pattern and
rhyme. Explain your imagery choices when asked. Keep drafts in drafts/ as
markdown files, one poem per file.
```

## Configuration

Set a default role in `config.toml`:

```toml
role = "writer"
```

Roles combine well with profiles, so one profile can bundle a persona with a model and other settings. Profiles are per-file (`$CODEX_HOME/<name>.config.toml`, top-level keys; the legacy `[profiles.<name>]` table syntax is rejected):

```toml
# ~/.codex/research.config.toml
role = "researcher"
model = "deepseek-reasoner"
```

Then launch with `codex --profile research`. The role name is the file stem of the role markdown, so `~/.codex/roles/ruyi.md` is selected with `role = "ruyi"`.

## How it works (and limits)

- The role spec is injected each turn as a developer message wrapped in `<role_spec>` tags, after the base instructions. Switching or clearing the role mid-session takes effect on the next turn.
- Roles deliberately do **not** replace the base instructions. If you want to fully replace the system prompt, use the (unsupported, sharp-edged) `model_instructions_file` config key, which points at a file whose contents become the entire base instructions — note that this strips harness guidance the model may rely on for tool calling.
- Review subagents (`/review`) always keep the stock coding persona, regardless of the active role.
- Models that are heavily specialized for coding may follow role prompts less faithfully than general-purpose models.

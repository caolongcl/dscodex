# Codex 指令系统文档

> 本文档枚举并说明 Codex 项目中所有向模型注入的指令（instructions/context fragments）。
> 架构与编排管线（如何选择 base instructions、片段如何组装/增量注入/随历史重放、如何落到 API 与 DeepSeek 代理）见姊妹篇 [prompt-system.md](prompt-system.md)。

## 概述

Codex 的指令系统基于 **上下文片段（Context Fragments）** 架构。所有指令通过 `ContextualUserFragment` trait 统一注入到模型的对话上下文中。每个片段定义了：

- **role**：消息角色（`user` 或 `developer`）
- **markers**：开闭标签，用于在历史消息中识别/过滤该片段
- **body**：片段内容

指令在 turn 开始时被收集并组装到 prompt 中，作为系统/用户消息发送给模型。

---

## 指令清单

### 1. BaseInstructions（基础指令）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/protocol/src/prompts/base_instructions/default.md` |
| **Role** | `developer`（作为 system 指令） |
| **标签** | 无标签标记（直接作为 `instructions` 字段传给 Responses API） |
| **触发条件** | 始终注入 |

**功能**：Codex 的核心系统提示（system prompt）。定义了：
- Codex CLI 的身份和能力边界
- AGENTS.md 规范：文件作用域、优先级规则
- 响应文案规范：前言消息、计划工具使用
- 代码修改规范：apply_patch 用法、环境复原规则
- 行为边界：不可编辑 `.env`、不可覆盖已知的 shell 功能
- 工具调用和沙箱使用的基础规则

这是所有指令中最基础、最核心的部分。

---

### 2. PermissionsInstructions（权限指令）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/permissions_instructions.rs` |
| **Role** | `developer` |
| **标签** | 无标签（由 `<permissions_instructions>` 标签包裹在 body 内） |
| **触发条件** | 始终注入 |

**功能**：向模型描述当前的沙箱模式和审批策略。根据运行配置动态生成，内容包含：

- **沙箱模式**（SandboxMode）：
  - `danger_full_access`：无限制
  - `workspace_write`：可读文件，可写 `cwd` 和 `writable_roots`，其他需要审批
  - `read_only`：只读
- **审批策略**（AskForApproval）：
  - `never`：永远不请求审批
  - `unless_trusted`：除非命令在信任列表中
  - `on_failure`：仅在失败后请求
  - `on_request`：总是请求
- **精细审批控制**（GranularApprovalConfig）：控制哪些审批类别（sandbox_approval、rules、skill_approval、request_permissions、mcp_elicitations）需要提示用户或被自动拒绝
- **自动审查模式**（auto_review）：自动审查合规性
- **已批准的 command prefix 列表**

---

### 3. UserInstructions（用户指令 / AGENTS.md）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/user_instructions.rs` |
| **Role** | `user` |
| **标签** | `# AGENTS.md instructions for <path>...<INSTRUCTIONS>...</INSTRUCTIONS>` |
| **触发条件** | 当前工作目录及祖先目录中存在 AGENTS.md 时 |

**功能**：注入 AGENTS.md 中的开发者指令。范围是从 CWD 到根目录的所有 AGENTS.md 文件。模型被指示：
- 作用于该文件所在目录树的全部文件
- 更深层的 AGENTS.md 在冲突时优先
- 直接的系统/开发者/用户指令优先于 AGENTS.md

---

### 4. EnvironmentContext（环境上下文）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/environment_context.rs` |
| **Role** | `user` |
| **标签** | `<environment_context>...</environment_context>` |
| **触发条件** | 始终注入 |

**功能**：描述当前运行环境。包含：

- **cwd**：当前工作目录
- **shell**：当前使用的 shell
- **current_date**：当前日期
- **timezone**：时区
- **network**：网络访问状态（enabled/disabled）及代理配置
- **subagents**：子代理描述（AgentPath 和描述）
- 多环境支持：可以为不同的 `TurnEnvironment` 描述各自的 `cwd` 和 `shell`

---

### 5. CollaborationModeInstructions（协作模式指令）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/collaboration_mode_instructions.rs` |
| **Role** | `developer` |
| **标签** | `<collaboration_mode>...</collaboration_mode>` |
| **触发条件** | CollaborationMode 配置了 developer_instructions 且非空时 |

**功能**：注入当前协作模式的开发者指令。协作模式（如 "Plan"、"Default"）可以携带自定义的开发者指令，通过此片段注入到模型上下文中，指导模型的协作行为。

---

### 6. SkillInstructions（技能指令）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/skill_instructions.rs` |
| **Role** | `user` |
| **标签** | `<skill>...</skill>` |
| **触发条件** | 当有活动技能（Skill）被激活时，每个技能注入一个 |

**功能**：注入每个已安装/激活技能的 `SKILL.md` 内容。格式为：

```
<skill>
<name>{skill_name}</name>
<path>{skill_path}</path>
{skill_contents}
</skill>
```

这是技能系统的核心机制——通过指令注入让模型了解特定领域的工作流。

---

### 7. AvailableSkillsInstructions（可用技能列表）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/available_skills_instructions.rs` |
| **Role** | `developer` |
| **标签** | `<skills_instructions>...</skills_instructions>` |
| **触发条件** | 当会话中注册了可用技能时 |

**功能**：向模型列出所有可用技能及其描述。告诉模型：
- 可用技能的名称和功能描述
- 技能使用方式：打开 SKILL.md 读取工作流
- 触发规则：用户提到技能名时必须使用
- 多技能协调顺序

---

### 8. AppsInstructions（应用/连接器指令）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/apps_instructions.rs` |
| **Role** | `developer` |
| **标签** | `<apps_instructions>...</apps_instructions>` |
| **触发条件** | 至少有一个应用连接器已启用且可访问时 |

**功能**：告诉模型如何调用应用连接器（Apps/Connectors）：
- 应用可以通过 `[$app-name](app://{connector_id})` 格式在用户消息中显式触发
- 应用等价于 MCP 服务器中的一组 MCP 工具
- 应用工具可以通过 `tool_search` 延迟加载

---

### 9. PluginInstructions（插件指令）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/plugin_instructions.rs` |
| **Role** | `developer` |
| **标签** | 无标签 |
| **触发条件** | 有活动插件时注入其指令文本 |

**功能**：注入插件的自定义指令文本。告诉模型如何使用特定的插件功能。

---

### 10. AvailablePluginsInstructions（可用插件列表）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/available_plugins_instructions.rs` |
| **Role** | `developer` |
| **标签** | `<plugins_instructions>...</plugins_instructions>` |
| **触发条件** | 至少有一个插件已启用时 |

**功能**：列出所有已启用插件的清单和描述。指导模型：
- 插件是技能、MCP 服务器和应用的本地捆绑包
- 技能以 `plugin_name:skill_name` 格式前缀
- 通过底层技能和 MCP 工具来使用插件能力

---

### 11. GoalContext（目标上下文）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/goal_context.rs` |
| **Role** | `user` |
| **标签** | `<goal_context>...</goal_context>` |
| **触发条件** | 当有活跃的运行期目标（goal）时 |

**功能**：注入运行期目标的提示文本。用于指导模型在长时间任务中保持对目标的关注。目标是运行时拥有的，不由用户直接设置。

---

### 12. PersonalitySpecInstructions（人格指令）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/personality_spec_instructions.rs` |
| **Role** | `developer` |
| **标签** | `<personality_spec>...</personality_spec>` |
| **触发条件** | 用户请求更改沟通风格/人格时 |

**功能**：当用户要求模型改变沟通风格（人格）时，注入新的人格规范。告诉模型后续消息应遵循指定的人格。

---

### 12b. RoleInstructions（角色指令）【本 fork 新增】

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/role_instructions.rs` |
| **Role** | `developer` |
| **标签** | `<role_spec>...</role_spec>` |
| **触发条件** | 配置了非 default 角色（`role` 配置键 / `/role` 命令）时，每轮注入 |

**功能**：注入当前激活角色（writer/researcher/自定义）的 persona 文本，重塑模型的身份与工作方式，但明确声明不覆盖基础指令中的工具、沙箱、审批等系统级规则。角色来源：内置预设（`codex-rs/protocol/src/prompts/roles/*.md`）或 `$CODEX_HOME/roles/<name>.md`（同名可覆盖内置，`default` 除外）。会话中切换角色经 ThreadSettingsOverrides → `build_role_update_item`（`context_manager/updates.rs`）发差量；review 子代理刻意不携带角色。

---

### 13. ImageGenerationInstructions（图片生成指令）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/image_generation_instructions.rs` |
| **Role** | `developer` |
| **标签** | 无标签 |
| **触发条件** | 图片生成功能可用时 |

**功能**：告诉模型生成的图片保存位置和命名规范。指示模型如需在其他路径使用图片应复制而不是移动。

---

### 14. ModelSwitchInstructions（模型切换指令）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/model_switch_instructions.rs` |
| **Role** | `developer` |
| **标签** | `<model_switch>...</model_switch>` |
| **触发条件** | 当会话从另一个模型切换到当前模型时 |

**功能**：当用户之前使用了不同的模型时，注入此片段通知模型切换，并提供继续对话的说明。

---

### 15. RealtimeStartInstructions（实时会话开始指令）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/realtime_start_instructions.rs` |
| **内容源** | `prompts/realtime/realtime_start.md` |
| **Role** | `developer` |
| **标签** | `<realtime_conversation>...</realtime_conversation>` |
| **触发条件** | 实时语音会话开始时 |

**功能**：通知模型实时语音会话已开始。告诉模型：
- 是后端执行者，通过中介与用户通信
- 用户可能不直接与模型对话，响应可能被摘要
- 消息来自语音转写，可能有识别错误或缺少标点
- 保持简洁、以行动为导向

---

### 16. RealtimeEndInstructions（实时会话结束指令）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/realtime_end_instructions.rs` |
| **内容源** | `prompts/realtime/realtime_end.md` |
| **Role** | `developer` |
| **标签** | `<realtime_conversation>...</realtime_conversation>` |
| **触发条件** | 实时语音会话结束时 |

**功能**：通知模型实时会话已结束。后续输入恢复为文本模式，不再假定识别错误或缺少标点。

---

### 17. RealtimeStartWithInstructions（自定义实时会话开始指令）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/realtime_start_with_instructions.rs` |
| **Role** | `developer` |
| **标签** | `<realtime_conversation>...</realtime_conversation>` |
| **触发条件** | 实时会话开始时且有自定义指令时 |

**功能**：与 RealtimeStartInstructions 功能相似，但内容是动态提供的自定义指令文本，而非读取静态文件。

---

### 18. TurnAborted（轮次中止通知）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/turn_aborted.rs` |
| **Role** | `user` |
| **标签** | `<turn_aborted>...</turn_aborted>` |
| **触发条件** | 用户中断了上一轮执行时 |

**功能**：当用户中断上一个 turn 时，注入此片段通知模型。告诉模型：
- 中断是用户有意的操作
- 正在运行的 unified exec 进程可能仍在后台运行
- 被中止的工具/命令可能已部分执行

---

### 19. UserShellCommand（用户 Shell 命令结果）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/user_shell_command.rs` |
| **Role** | `user` |
| **标签** | `<user_shell_command>...</user_shell_command>` |
| **触发条件** | 用户在对话中执行了 shell 命令（通过 `/user-shell` 功能） |

**功能**：记录用户在对话中执行的 shell 命令的结果。格式包含命令内容、退出码、持续时间和输出。让模型了解用户在终端中执行的操作。

---

### 20. HookAdditionalContext（钩子附加上下文）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/hook_additional_context.rs` |
| **Role** | `developer` |
| **标签** | 无标签 |
| **触发条件** | 有 hooks 注册了额外的上下文文本时 |

**功能**：允许 hooks 系统向开发者指令区域注入自定义文本。用于在不修改核心指令代码的情况下扩展现有行为。

---

### 21. GuardianFollowupReviewReminder（Guardian 后续审查提醒）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/guardian_followup_review_reminder.rs` |
| **Role** | `developer` |
| **标签** | 无标签 |
| **触发条件** | Guardian 安全审查进行后续评估时 |

**功能**：Guardian 安全系统在审查历史决定时使用的指令。告诉模型：
- 使用先前的审查作为上下文，而非约束性先例
- 遵循工作区策略
- 如果用户在被告知具体风险后明确批准了之前被拒绝的操作，应设为"allow"

---

### 22. ApprovedCommandPrefixSaved（已批准命令前缀保存通知）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/approved_command_prefix_saved.rs` |
| **Role** | `developer` |
| **标签** | 无标签 |
| **触发条件** | 用户批准了一个新的命令前缀规则时 |

**功能**：通知模型新的命令前缀已被批准保存。格式为："Approved command prefix saved:\n{prefixes}"。

---

### 23. NetworkRuleSaved（网络规则保存通知）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/network_rule_saved.rs` |
| **Role** | `developer` |
| **标签** | 无标签 |
| **触发条件** | 用户批准/拒绝了网络访问规则时 |

**功能**：通知模型新的网络规则（allow 或 deny）已保存到 execpolicy 中。例如："Allowed network rule saved in execpolicy (allowlist): example.com"。

---

### 24. SubagentNotification（子代理通知）

| 属性 | 值 |
|------|-----|
| **文件** | `codex-rs/core/src/context/subagent_notification.rs` |
| **Role** | `user` |
| **标签** | `<subagent_notification>...</subagent_notification>` |
| **触发条件** | 子代理（subagent）状态发生变化时 |

**功能**：当子代理的状态变化时通知模型。包含 agent_path 和 status（如 running、completed 等），使用 JSON 格式序列化。

---

### 25–27. Legacy Fragments（遗留片段）

| 名称 | 文件 | 功能 |
|------|------|------|
| `LegacyApplyPatchExecCommandWarning` | `context/legacy_apply_patch_exec_command_warning.rs` | 不再生成，仅用于从旧会话消息中过滤识别此模式 |
| `LegacyModelMismatchWarning` | `context/legacy_model_mismatch_warning.rs` | 不再生成，仅用于过滤旧会话中的账户风险警告 |
| `LegacyUnifiedExecProcessLimitWarning` | `context/legacy_unified_exec_process_limit_warning.rs` | 不再生成，仅用于过滤旧会话中的进程限制警告 |

这些遗留片段不注入新内容，仅实现 `matches_text()` 以在加载旧会话历史时识别和过滤掉不再相关的消息。

---

## 指令注入架构

### 注入流程

```
Turn 开始
    │
    ├── 收集静态片段：
    │   ├── BaseInstructions (始终注入)
    │   ├── PermissionsInstructions (始终注入)
    │   ├── EnvironmentContext (始终注入)
    │   ├── CollaborationModeInstructions (有条件)
    │   ├── RoleInstructions (有条件，本 fork 新增)
    │   ├── AvailableSkillsInstructions (有条件)
    │   ├── AvailablePluginsInstructions (有条件)
    │   ├── AppsInstructions (有条件)
    │   └── ImageGenerationInstructions (有条件)
    │
    ├── 收集动态片段（按需）：
    │   ├── SkillInstructions (每个激活的技能)
    │   ├── UserInstructions (每个 AGENTS.md)
    │   ├── GoalContext (有活跃目标时)
    │   ├── ModelSwitchInstructions (模型切换时)
    │   ├── RealtimeStart/End (实时会话)
    │   ├── TurnAborted (中断后)
    │   ├── UserShellCommand (用户 shell 后)
    │   └── SubagentNotification (子代理事件)
    │
    └── 合并为 developer/user 消息发送给模型
```

### 片段注册和识别

`contextual_user_message.rs` 维护了一个 **片段注册表**（`CONTEXTUAL_USER_FRAGMENTS`），用于在旧会话消息中识别和过滤已知的指令片段。注册表通过 `FragmentRegistrationProxy` 和 `type_markers()` 实现基于文本模式匹配。

### 关键设计原则

1. **片段单一职责**：每个 `ContextualUserFragment` 实现只负责一种类型的指令
2. **标记可识别性**：大多数片段通过唯一的开闭标签来标记，使得可以在历史消息中识别和过滤
3. **按需注入**：片段根据运行条件动态决定是否注入，避免冗余上下文
4. **内容与渲染分离**：提示文本放在 `prompts/` 目录的 `.md` 文件中，通过 `include_str!` 编译时加载

---

## 常见问答

### Q: 如何添加一个新的指令片段？
A: 在 `codex-rs/core/src/context/` 下创建新文件，实现 `ContextualUserFragment` trait，在 `mod.rs` 中注册，然后在合适的位置构造并插入。

### Q: 指令片段和系统提示有什么区别？
A: BaseInstructions 是系统级提示（通过 Responses API 的 `instructions` 字段）；其他片段作为上下文消息（user/developer role）注入到对话中。

### Q: 如何让指令对模型可见但不在 UI 中显示？
A: 使用 `developer` role（如 CollaborationModeInstructions），或使用不渲染标签的片段类型。

### Q: 如何更新审批策略的提示文本？
A: 在 `prompts/permissions/approval_policy/` 和 `prompts/permissions/sandbox_mode/` 下编辑对应的 `.md` 文件，然后重新编译。

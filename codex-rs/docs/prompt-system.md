# Codex 提示词系统全景

> 本文档回答三个问题：一次发给模型的请求里到底有什么；这些内容是如何被编排、注入、增量维护的；每一段提示词的作用与可定制点。
>
> 配套文档：[instructions.md](instructions.md) 是全部 27+ 种上下文片段的逐项目录（每个片段的标签、角色、触发条件）；本文讲的是它们之上的**架构与管线**。文中 `file:line` 以当前 fork 的 `dev` 分支为准。

---

## 1. 一次模型请求的解剖

Codex 走 OpenAI Responses API（本 fork 中 DeepSeek 经由内置代理翻译，见 §9）。每次采样请求的结构：

```
ResponsesApiRequest
├── instructions        ← 第一层：base instructions（系统提示，顶层字段，不在消息列表里）
├── input[]             ← 第二层：完整对话历史的重放，其中混着：
│     ├── developer 消息（上下文片段 bundle：权限、协作模式、人格、角色、技能列表…）
│     ├── user 消息（AGENTS.md、环境上下文、真实用户输入…）
│     ├── assistant 消息、reasoning、function_call / function_call_output
├── tools[]             ← 第三层：工具名 + 描述 + 参数 schema（描述本身就是提示词）
├── reasoning           ← effort / summary 设置
├── prompt_cache_key    ← 缓存键（按会话窗口派生）
├── parallel_tool_calls / stream / store / include / service_tier ...
```

对应代码：`Prompt` 结构（core/src/client_common.rs:18-39）→ `build_responses_request`（core/src/client.rs:755-817）→ `ResponsesApiRequest`（codex-api/src/common.rs:183-203）。

理解全局的关键一句话：**base instructions 是每次请求都完整发送的顶层字段；上下文片段不是"每轮重新拼"的，而是在恰当的时机作为消息写进对话历史，之后随历史整体重放**（见 §5，这是最容易误解的部分）。

---

## 2. 第一层：base instructions（系统提示从哪来）

### 2.1 选择优先级链

会话创建时一次性确定（core/src/session/mod.rs:567-572）：

```
1. config.base_instructions            ← model_instructions_file / instructions 配置键
2. 恢复会话时 rollout 里存的 base instructions（保证 resume 后提示词一致）
3. model_info.get_model_instructions(personality)   ← 正常路径：按模型选
```

其中第 1 级在配置加载时解析（core/src/config/mod.rs:3244-3253）：`model_instructions_file`（文件路径，整体替换）→ `instructions`（内联字符串）。这是"整体换掉系统提示"的唯一入口，锋利且无文档保护——换掉后 harness 的工具/沙箱约定一并丢失。

### 2.2 按模型选择：models.json 模板系统

`get_model_instructions`（protocol/src/openai_models.rs:449-468）的逻辑：

- 模型元数据（`models-manager/models.json`，编译期 `include_str!` 打包，启动时可在线刷新）里若有 `model_messages.instructions_template`，用模板，并把 `{{ personality }}` 占位符替换为对应人格文本；
- 否则用该模型的 `base_instructions` 字符串；
- 都没有则回退到 `BASE_INSTRUCTIONS_DEFAULT` = `protocol/src/prompts/base_instructions/default.md`。

人格（personality）因此是**烘焙进系统提示**的，模板形如（models-manager/src/model_info.rs:110-123）：

```
You are Codex, a coding agent based on GPT-5. ...   ← 身份头
{{ personality }}                                    ← Friendly/Pragmatic 文本替换于此
<base instructions 正文>
```

gpt-5.4/5.5/5.3-codex 等在 models.json 里带完整模板与三种人格变量；`gpt-5.2-codex` 有一份代码内置的本地兜底。对不支持模板的模型，人格退化为运行时注入的 `<personality_spec>` 片段（见 instructions.md §12）。

**本 fork 的 role 与此对照**：role 刻意不走烘焙路线，而是独立的 `<role_spec>` developer 片段（instructions.md §12b），原因是烘焙路径是 OpenAI 模型专属的（依赖 models.json 模板），而片段注入对任何 provider 生效且不破坏 harness 粘合层。

### 2.3 默认 base instructions 的内容结构

`default.md` 约 275 行 / ~7000 token，节结构与各节作用：

| 节 | 作用 |
|---|---|
| 身份前言 | "You are a coding agent running in the Codex CLI, a terminal-based coding assistant. Codex CLI is an open source project led by OpenAI."——**模型中立**，不声明底层模型是什么；"based on GPT-5" 的措辞只出现在 gpt-5.x 专属的 models.json 模板/人格模板头里，第三方模型不会收到 |
| How you work / Personality | 默认语气：简洁、直接、友好 |
| AGENTS.md spec | 作用域与优先级规则：直接指令 > 深层 AGENTS.md > 浅层 |
| Responsiveness / preamble | 工具调用前的"我接下来要做什么"短消息规范 |
| Planning | update_plan 工具的使用时机、好/坏计划示例 |
| Task execution | 自主性边界、apply_patch 用法、不擅自 commit、编码风格守则 |
| Validating your work | 测试哲学：从专项测到全量，格式化/lint 的时机 |
| Ambition vs. precision | 新项目可大胆、存量项目要克制 |
| Sharing progress updates | 长任务的进度播报频率与语气 |
| Presenting your work | 最终回答排版规范：标题、列表、等宽、文件引用格式 |
| Tool Guidelines | shell 偏好（rg 优先）、update_plan 机制 |

仓库里的 `core/gpt_5_1_prompt.md`、`core/gpt_5_2_prompt.md`、`core/prompt_with_apply_patch_instructions.md` 是历史/测试用变体；生产路径上 apply_patch 的说明已迁移到工具描述（freeform 工具 + Lark 语法，见 §8），不再追加到 base instructions。

---

## 3. 第二层：上下文片段的统一抽象

所有运行时注入的提示词段落实现同一个 trait（context-fragments/src/fragment.rs:46）：

```rust
trait ContextualUserFragment {
    fn role(&self) -> &'static str;        // "developer" 或 "user"
    fn markers(&self) -> (&str, &str);     // 开闭标签，如 ("<role_spec>", "</role_spec>")
    fn body(&self) -> String;              // 内容
    fn render(&self) -> ResponseItem;      // 变成一条消息
}
```

标签（markers）的用途不是给模型看的装饰，而是**让系统能在历史里认出自己注入过的内容**：

- developer 片段靠前缀表识别：`CONTEXTUAL_DEVELOPER_PREFIXES`（core/src/event_mapping.rs:27-34）= `<permissions instructions>`、`<model_switch>`、协作模式标签、realtime 标签、`<personality_spec>`、`<role_spec>`；
- user 片段靠注册表 `CONTEXTUAL_USER_FRAGMENTS`（core/src/context/contextual_user_message.rs:46-58）逐个 `matches_text` 匹配（AGENTS.md、环境上下文、技能、turn_aborted 等 11 种）。

识别能力支撑两件事：回滚/fork 时把片段从历史里修剪掉（§5.3），以及从旧会话恢复时过滤已废弃的片段类型（instructions.md §25-27 的 Legacy 片段就只剩这个用途）。

role 的差异：`developer` 片段是"系统侧规则"（权限、模式、人格、角色），`user` 片段是"用户侧上下文"（AGENTS.md、环境、技能内容）——后者在模型眼里更接近用户提供的材料而非平台指令，优先级语义不同。

---

## 4. 编排：首轮的全量注入

会话第一轮（或基线被清除后，见 §5），`build_initial_context`（core/src/session/mod.rs:2755-2990）按固定顺序组装。**developer 段落先收集进一个数组，最后合并成一条 developer 消息**（guardian 例外，见下）：

| 序 | 段落 | 条件 | 为什么在这个位置 |
|---|---|---|---|
| 1 | ModelSwitchInstructions | 切过模型 | 模型专属指南必须最先读 |
| 2 | PermissionsInstructions | `include_permissions_instructions` | 沙箱/审批规则，所有行为的前提 |
| 3 | developer_instructions（配置键） | 非空 | 用户自定义的系统侧叠加 |
| 4 | CollaborationModeInstructions | `include_collaboration_mode_instructions` 且模式带指令 | Plan/Pair 等模式行为 |
| 5 | Realtime start/end | 实时语音会话 | |
| 6 | PersonalitySpecInstructions | Feature 开 + 未烘焙进 base | 沟通风格 |
| 7 | **RoleInstructions（fork 新增）** | 配置了非 default 角色 | persona 重塑，紧跟人格之后 |
| 8 | AppsInstructions | 有可用连接器 | 能力清单类 |
| 9 | AvailableSkillsInstructions | `include_skill_instructions` | 技能名+描述清单（不含正文） |
| 10 | PluginInstructions + 贡献者片段 | 有插件/扩展 | |

之后的 **contextual user 部分**（合并成一条 user 消息）：

| 段落 | 内容 |
|---|---|
| UserInstructions | 每个 AGENTS.md 一段：`# AGENTS.md instructions for <目录>` + `<INSTRUCTIONS>正文</INSTRUCTIONS>` |
| EnvironmentContext | `<environment_context>`：cwd、shell、日期、时区、网络开关与域名单、文件系统权限明细、子代理 |

两个特殊处理：

- **Guardian 隔离**（mod.rs:2805-2814）：guardian 审查子代理的策略提示不并入 developer bundle，保持独立顶层消息，避免与常规上下文混淆；
- **Multi-agent V2 usage hint**（mod.rs:2958-2981）：多代理模式下按"根代理/子代理"身份注入不同的协作守则，单独一条 developer 消息。

组装出的消息**写入对话历史**（`record_conversation_items`，mod.rs:3052），同时把当前 `TurnContextItem` 快照（cwd、模型、权限、人格、角色名等持久字段）写入 rollout 并存为内存基线 `reference_context_item`。

---

## 5. 增量机制：之后的每一轮发生什么

这是整个系统最精巧的部分。

### 5.1 全量 vs 差量的决策

每轮开始时（core/src/session/turn.rs:157 → mod.rs:3034-3064）：

```
reference_context_item 为 None ？
  ├─ 是 → build_initial_context()         全量注入（首轮 / 回滚后 / 某类压缩后）
  └─ 否 → build_settings_update_items()   只注入"变了的部分"
```

差量构建（core/src/context_manager/updates.rs:209-260）把上一轮的 `TurnContextItem` 与本轮 `TurnContext` 逐项对比，只为变化项生成片段：环境变了发 `<environment_context>`、权限变了发 `<permissions instructions>`、换模型发 `<model_switch>`、换人格发 `<personality_spec>`、换角色发 `<role_spec>`（`build_role_update_item`）。没变化就一条不发。

### 5.2 历史是"账本"，不是每轮重建

片段一旦写入历史就**留在那里**，每次请求随全量历史重放。一个真实例子（本 fork roles 集成测试 `thread_settings_switch_role_mid_session` 验证过的行为）：

```
轮1（无角色）   : 历史 = [初始上下文(无 role), u1, a1]
轮2（切 researcher）: 历史 += [<role_spec>researcher 片段, u2, a2]   ← 差量
轮3（切回 default） : 历史 += [u3, ...]                              ← 不发新片段
```

轮 3 的请求里仍能看到轮 2 的那条 `<role_spec>`——清除角色靠的是"不再有新的角色声明"+模型理解上下文时序，而不是篡改历史。这保证了**提示词前缀的字节级稳定**，是 prompt 缓存命中的前提。

### 5.3 什么时候基线会被清掉（强制全量重注入）

`trim_pre_turn_context_updates`（core/src/context_manager/history.rs:393-421）在**回滚/fork** 时从切点向前修剪连续的上下文片段；如果剪到一条"片段+非片段混合"的 developer 消息，无法干净拆分，就清空 `reference_context_item`——下一轮回到全量注入。压缩也分两种（§7）：轮前压缩清基线，轮中压缩重建基线。

### 5.4 重放与传输优化

每次请求在语义上是**无状态全量重放**（`sess.clone_history().for_prompt()`，turn.rs:216-222；重试时重新 clone 以带上新工具输出）。传输层再做优化：WebSocket 通道下若新请求是上一请求的纯前缀扩展，只发增量 + `previous_response_id`（client.rs:1030-1104）。`prompt_cache_key` 按会话窗口派生（client.rs:375-376），`core/tests/suite/prompt_caching.rs` 专门断言工具数组与 instructions 跨请求字节一致。

---

## 6. 压缩（compaction）

触发：每轮采样前的 token 阈值检查（turn.rs:149）或手动 `/compact`。

提示词：`SUMMARIZATION_PROMPT`（prompts/templates/compact/prompt.md，"你在执行 CONTEXT CHECKPOINT COMPACTION，为另一个 LLM 写交接摘要：进度、约束、下一步、关键数据"），可用 `compact_prompt` 配置键整体替换。压缩产物以 `SUMMARY_PREFIX`（"另一个模型已部分解决此问题，以下是它的摘要…"）开头作为替换历史。

两种基线处理（core/src/compact.rs:289-313）：

- **轮前/手动压缩**：`reference_context_item = None` → 下一轮全量重注入上下文（摘要里没有片段，必须重建）；
- **轮中压缩**：立即调 `build_initial_context` 把上下文插回"最后一条真实用户消息之前"，并重建基线——让正在进行的任务无缝继续。

---

## 7. 工具描述也是提示词

`tools[]` 数组里的每个工具的 `description` 和参数 schema 都是模型读的文本，体量不小：

- **shell**（`exec_command`，core/src/tools/handlers/shell_spec.rs:90-97）：PTY 语义说明，Windows 下追加专门指引；
- **apply_patch**（apply_patch_spec.rs:18-21）：freeform 工具，带完整 **Lark 语法定义**——补丁格式规范不在 base instructions 里，而是作为工具的 grammar 字段下发；
- **update_plan**（plan_spec.rs）："维护任务计划，至多一项 in_progress"；
- MCP/连接器工具：名称带服务器前缀，描述原样透传。

这就是为什么"整体替换 base instructions 不会让工具失灵，但会丢掉**何时用哪个工具**的策略指导"——机制在 tools 数组，策略在 base instructions，两层缺一不可。

---

## 8. 周边提示词表面

主对话之外，还有这些独立的提示词体系：

| 表面 | 提示词来源 | 说明 |
|---|---|---|
| `/review` | prompts/templates/review/rubric.md（88 行） | P0-P3 分级、评论语气、JSON 输出 schema；请求侧模板按目标（未提交/对比分支/单 commit）生成（prompts/src/review_request.rs）。review 子代理不继承主线程的 role |
| Guardian 自动审查 | core/src/guardian/prompt.rs + 策略文本 | 独立顶层 developer 消息（§4），带主线程转录摘要与重试原因 |
| 协作模式 | collaboration-mode-templates/templates/plan.md（129 行）、pair_programming.md | Plan 模式：三阶段（探环境→问意图→出 `<proposed_plan>`），禁止变更性操作，明确"与 update_plan 工具无关" |
| 多代理编排 | core/templates/collab/experimental_prompt.md、templates/agents/orchestrator.md | 子代理互不干扰、并行优先、等子代理完成再收尾 |
| Skills | `$CODEX_HOME/skills/<name>/SKILL.md` | 两级注入：developer 消息列"名称+描述"清单（§4 第 9 行），模型按需打开 SKILL.md，激活后正文以 `<skill>` user 片段进入上下文 |
| AGENTS.md | core/src/agents_md.rs | 发现：项目根（.git 或 project_root_markers）→ cwd 逐级收集，不越过项目根；`AGENTS.override.md` 本地覆盖优先；受 `project_doc_max_bytes` 限制；多文件用 `--- project-doc ---` 分隔拼接 |
| 权限/沙箱说明 | prompts/templates/permissions/{sandbox_mode,approval_policy}/*.md | PermissionsInstructions 片段的正文来源，按当前模式拼装 |
| Goals / Realtime | prompts/templates/{goals,realtime}/*.md | 长任务目标续期、预算耗尽通知；语音会话开始/结束须知 |
| **Roles（fork 新增）** | protocol/src/prompts/roles/*.md + `$CODEX_HOME/roles/*.md` | 见 docs/roles.md 与 instructions.md §12b |

注：旧版 codex 的 `~/.codex/prompts/*.md` 自定义斜杠提示词机制在当前代码中**不存在**（TUI 的 `CustomPromptView` 只是 review 自定义说明的输入框），该职能由 skills 承接。

---

## 9. DeepSeek 代理路径上的提示词变换（本 fork）

`deepseek-proxy/src/translate.rs` 把 Responses 请求翻译为 Chat Completions，提示词相关的语义变化：

| Responses 侧 | DeepSeek 侧 | 影响 |
|---|---|---|
| `instructions` 顶层字段 | 开头的 `system` 消息 | 语义等价 |
| `developer` 角色消息 | 改写为 `system`（translate.rs:344-349） | **所有上下文片段（权限/人格/角色…）在 DeepSeek 眼里都是 system 消息**，权重高于 OpenAI 的 developer 语义 |
| `reasoning` 项 | 缓存后作为下一条 assistant 消息的 `reasoning_content` 回传 | thinking 模式硬性要求；缺失时补空串 |
| reasoning effort low/medium/high → high，xhigh/max → max | | DeepSeek 只有两档 |
| 非 function 工具（web_search 等） | 丢弃并告警 | |
| thinking 模式下 temperature/top_p | 置 null | DeepSeek 约束 |
| 流式回包 | reasoning_content → `reasoning_summary_*` 事件，文本/工具调用 delta 一一映射 | |

实践含义：给 DeepSeek 写自定义 role / AGENTS.md 时，可以假设它们以 system 级权重生效；而 base instructions 里 OpenAI 模型靠 RL 内化的行为（preamble 节奏、计划工具习惯），DeepSeek 全靠提示词字面遵循，写 role 时值得把期望行为说得更明确。

---

## 10. 定制点速查："想改 X → 动哪里"

| 想做什么 | 入口 | 层级 |
|---|---|---|
| 加项目级守则 | AGENTS.md / AGENTS.override.md | user 片段，叠加 |
| 换 persona（写作/科研…） | `/role`、`role = "..."`、`~/.codex/roles/*.md` | developer 片段，叠加（fork） |
| 调沟通风格 | `/personality`、`personality` 键 | 烘焙或片段 |
| 叠一段系统侧指令 | `developer_instructions` 键 | developer bundle 第 3 位 |
| 整体替换系统提示 | `model_instructions_file` / `instructions` 键 | 第一层，锋利 |
| 换压缩提示词 | `compact_prompt` 键 | 压缩表面 |
| 裁剪注入的片段 | `include_permissions_instructions` / `include_apps_instructions` / `include_collaboration_mode_instructions` / `include_environment_context` / skills 的 `include_instructions` | 编排开关 |
| 加领域工作流 | `$CODEX_HOME/skills/<name>/SKILL.md` | 两级注入 |
| 按场景打包以上一切 | `[profiles.<name>]` + `codex --profile` | 配置层 |

---

## 11. 源码阅读地图

| 想看什么 | 从这里进 |
|---|---|
| 片段 trait 与注册 | context-fragments/src/fragment.rs:46；core/src/context/contextual_user_message.rs:46 |
| 全量编排顺序 | core/src/session/mod.rs:2755 `build_initial_context` |
| 全量/差量决策 | core/src/session/mod.rs:3034 `record_context_updates_and_set_reference_context_item` |
| 差量构建 | core/src/context_manager/updates.rs:209 `build_settings_update_items` |
| 历史修剪/规范化 | core/src/context_manager/history.rs:393、event_mapping.rs:27 |
| base instructions 选择 | core/src/session/mod.rs:567；protocol/src/openai_models.rs:449 |
| 请求组装 | core/src/client.rs:755 `build_responses_request` |
| 传输增量 | core/src/client.rs:1030 `get_incremental_items` |
| 压缩 | core/src/compact.rs:289 |
| DeepSeek 翻译 | deepseek-proxy/src/translate.rs:58、:344 |
| 行为验证范例 | core/tests/suite/{roles,personality,collaboration_instructions,prompt_caching}.rs |

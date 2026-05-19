# System Prompt KV Cache 跨 Session 复用

## 目标

子 session（agent tool spawn 的 child）复用主 session 的系统提示词 KV cache，减少重复计算和 token 费用。

## 现状

- `build_system_prompt()` 输出单个字符串 → 单个 `LlmMessage::system()`
- Anthropic provider：整个系统提示词打成一个 system block，尾部一个 `cache_control`
- OpenAI Chat Completions：单个 system message 在 `messages` 数组中
- OpenAI Responses API：所有系统文本合并进 `instructions` 单字段
- OpenAI Compatible（prompt_cache_key）：hash(全部系统提示词 + 工具 + model)，精确匹配

## 方案

核心思路：把系统提示词从单个字符串拆成按 section 分组的多个 `LlmMessage::system()`。

```
当前：  [Identity + System + ... + ExtraInstructions]  →  1 个 LlmMessage::system()
改后：  [Identity..Communication]  →  LlmMessage::system()  ← 静态前缀，跨 session 复用
        [Environment..ToolSummary]  →  LlmMessage::system()  ← 半静态
        [Extensions..Extra]         →  LlmMessage::system()  ← 动态，子 session 不同
```

## 各 Provider 支持情况

| Provider | 能否复用 | 说明 |
|---|---|---|
| Anthropic | 可以 | 拆 block + cache_control 断点，前缀匹配 |
| OpenAI Chat Completions（官方 API） | 可以 | 拆 system message 后自动前缀缓存 |
| OpenAI Responses API | 不行 | 单 `instructions` 字段限制 |
| OpenAI Compatible（第三方 prompt_cache_key） | 不行 | 精确 hash 匹配，无前缀机制 |

## 任务列表

### Phase 1: 拆分系统提示词输出结构

- [ ] `PromptEngine` / `build_system_prompt()` 输出 `Vec<(PromptSectionGroup, String)>` 分组结构
  - Group::Static: Identity, System, TaskGuidelines, Communication
  - Group::SemiStatic: Environment, UserRules, ProjectRules
  - Group::Dynamic: ToolSummary, ExtensionPrompt (PlatformInstructions, Skills, Agents), ExtraInstructions
- [ ] 保持 `build_system_prompt()` 向后兼容（单个字符串拼接），新增 `build_system_prompt_sections()` 返回分组

### Phase 2: 消息流改造

- [ ] `TurnState::new()` 接收分组后的多个 `LlmMessage::system()`，替换当前单个 system message
- [ ] `session_setup.rs` / `turn_stages.rs` 适配分组输出
- [ ] `turn_runner.rs` 的 prepare 阶段（system_messages 分区）验证多 system message 正常工作

### Phase 3: Anthropic Provider

- [ ] `convert_messages()` 中对每个 system block 加 `cache_control: {"type": "ephemeral"}`
- [ ] 确认单请求 cache breakpoints 不超过 Anthropic 限制（最多 4 个，当前 3 组没问题）
- [ ] 验证：同 session 跨 turn 缓存命中 + 子 session 静态前缀缓存命中

### Phase 4: OpenAI Chat Completions

- [ ] 多 system message 自然排在 `messages` 数组前面，确认 OpenAI 自动前缀缓存生效
- [ ] `prompt_cache_key` 保持不变（仍用于第三方 provider 精确缓存）

### Phase 5: 测试 & 验证

- [ ] 单元测试：`build_system_prompt_sections()` 分组正确性
- [ ] 单元测试：Anthropic `convert_messages()` 多 system block 各带 `cache_control`
- [ ] 集成测试：主 turn → spawn agent child → 检查 Anthropic API 日志确认 cache 命中
- [ ] 回归测试：单 session 多 turn 行为不变

## 已知限制

- Anthropic ephemeral cache TTL 5 分钟，父 turn 结束后太久启动子 session 会 miss
- 子 session 如果 working_dir 与父不同，Environment 段也变了，复用前缀缩短为 Identity → Communication
- OpenAI Responses API 和第三方 prompt_cache_key 无法支持前缀复用
- Anthropic 单请求最多 4 个 cache breakpoints

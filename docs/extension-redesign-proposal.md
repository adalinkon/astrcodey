# Extension System Redesign Proposal

> 状态：草案 v2，待讨论
> 日期：2026-05-12

---

## 0. 愿景与设计原则

### 0.1 演化方向

astrcode 从 coding agent 演化为**全能 agent 平台**。插件系统需要支撑：
- 当前：5-10 个内置扩展，Rust 实现
- 未来：几十个社区扩展，第三方来源，需要隔离

### 0.2 设计原则

1. **插件只提出影响，主流程决定如何应用**
   - 插件返回 decision / patch，由宿主统一应用
   - 插件不持有 `&mut` 会话状态

2. **Context 是 plain struct，不是 trait**
   - 消除委托样板（当前 ProviderMessagesContext 170 行）
   - 天然可序列化（IPC / Wasm 就绪）
   - 测试直接构造 struct

3. **传输层可替换**
   - Extension trait + Handler trait 是协议层
   - Phase 1：in-process（`Arc<dyn>` 直接调用）
   - Phase 2：进程隔离（IPC stub 替换 `Arc<dyn>`）
   - 插件代码不需要改

4. **每个 hook 有独立的 Context struct + Result 枚举**
   - 编译期约束：`PreToolUseHandler` 不可能返回 `ModifiedMessages`
   - 消除当前 dispatch 里的 "ignore" 分支

---

## 1. 现状诊断

### 1.1 核心问题

| 问题 | 现状 | 影响 |
|------|------|------|
| Extension trait 方法太多 | 10 个方法，混合能力注册和事件处理 | 扩展作者认知负担高 |
| ExtensionContext 上帝接口 | 23 方法，7 种职责 | 任何包装器必须 100% 委托 |
| HookEffect 万能枚举 | 9 变体，每个事件只接受 1-2 个 | dispatch 充满 ignore 分支 |
| ProviderMessagesContext 样板 | 170 行委托，只为改写 2 个方法 | 维护负担 |
| 事件-效果耦合隐式 | 运行时 warn，不是编译错误 | 容易写错 |

### 1.2 当前架构

```
Extension trait (10 methods)
  ├── id()
  ├── hook_subscriptions() / on_event()     ← 事件处理
  ├── tools() / execute_tool()              ← 能力注册
  ├── slash_commands() / execute_command()   ← 能力注册
  └── tool_prompt_metadata()                ← 能力注册

ExtensionContext trait (23 methods)
  ├── 会话信息：session_id, working_dir, model_selection, session_history, system_prompt
  ├── 配置：config_value
  ├── 事件：emit_custom_event, event_bus, broadcast
  ├── 工具：find_tool, register_tool, drain_registered_tools
  ├── Hook 输入：pre_tool_use_input, post_tool_use_input, ...
  ├── 会话操作：send_message, set_model, compact
  └── 工具：snapshot, log_warn

HookEffect (9 variants): Allow | Block | ModifiedInput | ModifiedResult |
  ModifiedMessages | AppendMessages | ModifiedOutput | PromptContributions | CompactContributions
```

---

## 2. 提案：核心设计

### 2.1 Extension trait — 2 个方法

```rust
/// 扩展的入口。只需声明身份 + 注册能力。
///
/// 借鉴 pi-mono 的 `(api) => void` 工厂模式：
/// 扩展通过 registrar 声明自己提供什么，而不是实现一个大型 trait。
pub trait Extension: Send + Sync {
    fn id(&self) -> &str;

    /// 一次性调用。扩展通过 registrar 注册工具、命令和事件处理器。
    fn register(&self, reg: &mut Registrar);
}
```

### 2.2 Registrar — 强类型注册入口

```rust
/// 扩展能力注册器。register() 调用期间有效。
///
/// 方法签名 = 协议 V1。未来进程隔离时，映射到 IPC 消息。
pub struct Registrar<'a> { /* internal */ }

impl<'a> Registrar<'a> {
    // ── 工具 ──
    pub fn tool(&mut self, def: ToolDefinition, handler: Arc<dyn ToolHandler>);
    pub fn tools(&mut self, defs: Vec<ToolDefinition>);
    pub fn tool_metadata(&mut self, meta: HashMap<String, ToolPromptMetadata>);

    // ── 命令 ──
    pub fn command(&mut self, cmd: SlashCommand, handler: Arc<dyn CommandHandler>);

    // ── 事件订阅（类型化，每个 hook 独立）──
    pub fn on_pre_tool_use(&mut self, mode: HookMode, handler: Arc<dyn PreToolUseHandler>);
    pub fn on_post_tool_use(&mut self, mode: HookMode, handler: Arc<dyn PostToolUseHandler>);
    pub fn on_provider(&mut self, event: ProviderEvent, mode: HookMode,
                       handler: Arc<dyn ProviderHandler>);
    pub fn on_prompt_build(&mut self, handler: Arc<dyn PromptBuildHandler>);
    pub fn on_compact(&mut self, event: CompactEvent, handler: Arc<dyn CompactHandler>);

    // ── 通用生命周期（SessionStart, TurnStart 等）──
    pub fn on_event(&mut self, event: ExtensionEvent, mode: HookMode,
                    handler: Arc<dyn LifecycleHandler>);
}
```

### 2.3 Handler Traits — 每个 hook 一个

每个 handler trait 只有**一个 async 方法**。

#### 工具 Hook

```rust
#[async_trait]
pub trait PreToolUseHandler: Send + Sync {
    async fn handle(&self, ctx: PreToolUseContext)
        -> Result<PreToolUseResult, ExtensionError>;
}

#[async_trait]
pub trait PostToolUseHandler: Send + Sync {
    async fn handle(&self, ctx: PostToolUseContext)
        -> Result<PostToolUseResult, ExtensionError>;
}
```

#### Provider Hook

```rust
#[async_trait]
pub trait ProviderHandler: Send + Sync {
    async fn handle(&self, ctx: ProviderContext)
        -> Result<ProviderResult, ExtensionError>;
}

pub enum ProviderEvent {
    BeforeRequest,
    AfterResponse,
}
```

#### Prompt / Compact

```rust
#[async_trait]
pub trait PromptBuildHandler: Send + Sync {
    async fn handle(&self, ctx: PromptBuildContext)
        -> Result<PromptContributions, ExtensionError>;
}

#[async_trait]
pub trait CompactHandler: Send + Sync {
    async fn handle(&self, ctx: CompactContext)
        -> Result<CompactResult, ExtensionError>;
}
```

#### 通用生命周期

```rust
#[async_trait]
pub trait LifecycleHandler: Send + Sync {
    async fn handle(&self, ctx: LifecycleContext) -> Result<HookResult, ExtensionError>;
}
```

#### 工具 / 命令

```rust
#[async_trait]
pub trait ToolHandler: Send + Sync {
    async fn execute(&self, args: serde_json::Value, ctx: &ToolExecutionContext)
        -> Result<ToolResult, ExtensionError>;
}

#[async_trait]
pub trait CommandHandler: Send + Sync {
    async fn execute(&self, args: &str, working_dir: &str, ctx: &CommandContext)
        -> Result<ExtensionCommandResult, ExtensionError>;
}
```

### 2.4 Context — Plain Struct（关键设计）

**这是与之前提案的核心区别。** Context 是 owned struct，不是 trait。

```rust
// ── 工具 Hook Context ──

pub struct PreToolUseContext {
    pub session_id: String,
    pub working_dir: String,
    pub model: ModelSelection,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    pub config: HashMap<String, String>,
    pub available_tools: Vec<ToolDefinition>,
}

pub struct PostToolUseContext {
    pub session_id: String,
    pub working_dir: String,
    pub model: ModelSelection,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    pub tool_result: ToolResult,
    pub config: HashMap<String, String>,
}

// ── Provider Hook Context ──

pub struct ProviderContext {
    pub session_id: String,
    pub working_dir: String,
    pub model: ModelSelection,
    pub messages: Vec<LlmMessage>,
    pub session_history: Vec<LlmMessage>,
    pub system_prompt: Option<String>,
    pub config: HashMap<String, String>,
}

// ── Prompt Build Context ──

pub struct PromptBuildContext {
    pub session_id: String,
    pub working_dir: String,
    pub model: ModelSelection,
    pub config: HashMap<String, String>,
}

// ── Compact Context ──

pub struct CompactContext {
    pub session_id: String,
    pub working_dir: String,
    pub trigger: CompactTrigger,
    pub message_count: usize,
    // PostCompact 额外字段
    pub pre_tokens: Option<usize>,
    pub post_tokens: Option<usize>,
    pub summary: Option<String>,
}

// ── 通用生命周期 Context ──

pub struct LifecycleContext {
    pub session_id: String,
    pub working_dir: String,
    pub model: ModelSelection,
    pub config: HashMap<String, String>,
}

// ── 命令 Context ──

pub struct CommandContext {
    pub session_id: String,
    pub working_dir: String,
    pub model: ModelSelection,
    pub config: HashMap<String, String>,
}
```

**Plain struct 的好处**：
- **0 行委托样板** — ProviderMessagesContext 170 行完全消失
- **天然可序列化** — Phase 2 进程隔离时直接打包传输
- **测试友好** — 直接 `PreToolUseContext { session_id: "test".into(), ... }`
- **清晰所有权** — 没有 `&dyn` 引用的生命周期问题
- **Wasm 就绪** — struct 可以直接映射到 WIT 类型

### 2.5 Result — 类型化枚举

每个 hook 的返回类型只包含对该 hook 有意义的变体。

```rust
// 通用结果（只有 Allow / Block）
pub enum HookResult {
    Allow,
    Block { reason: String },
}

// PreToolUse 结果
pub enum PreToolUseResult {
    Allow,
    Block { reason: String },
    ModifyInput { tool_input: serde_json::Value },
}

// PostToolUse 结果
pub enum PostToolUseResult {
    Allow,
    Block { reason: String },
    ModifyResult { content: String },
}

// Provider 结果
pub enum ProviderResult {
    Allow,
    Block { reason: String },
    ReplaceMessages { messages: Vec<LlmMessage> },
    AppendMessages { messages: Vec<LlmMessage> },
}

// Compact 结果
pub enum CompactResult {
    Allow,
    Block { reason: String },
    Contributions(CompactContributions),
}
```

**对比当前**：1 个 `HookEffect` 枚举 9 变体 → 5 个类型化枚举，每个 2-4 变体。

### 2.6 内部 ExtensionRecord

ExtensionRunner 内部持有从 `register()` 收集的结构化记录：

```rust
struct ExtensionRecord {
    id: String,
    // 类型化的 handler 存储
    pre_tool_use: Vec<(HookMode, Arc<dyn PreToolUseHandler>)>,
    post_tool_use: Vec<(HookMode, Arc<dyn PostToolUseHandler>)>,
    provider_hooks: HashMap<ProviderEvent, Vec<(HookMode, Arc<dyn ProviderHandler>)>>,
    prompt_build: Option<Arc<dyn PromptBuildHandler>>,
    compact_hooks: HashMap<CompactEvent, Arc<dyn CompactHandler>>,
    lifecycle: HashMap<ExtensionEvent, Vec<(HookMode, Arc<dyn LifecycleHandler>)>>,
    // 能力
    tools: Vec<(ToolDefinition, Option<Arc<dyn ToolHandler>>)>,
    commands: Vec<(SlashCommand, Arc<dyn CommandHandler>)>,
    tool_metadata: HashMap<String, ToolPromptMetadata>,
}
```

---

## 3. 内置扩展迁移示例

### 当前写法

```rust
struct SkillExtension;

impl Extension for SkillExtension {
    fn id(&self) -> &str { "astrcode-skill" }
    fn hook_subscriptions(&self) -> Vec<HookSubscription> { vec![] }
    fn tools(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition { name: "skill".into(), ... }]
    }
    async fn execute_tool(&self, name: &str, args: Value, dir: &str, ctx: &ToolExecutionContext)
        -> Result<ToolResult, ExtensionError>
    {
        // ... 执行逻辑
    }
    // 其余 6 个方法使用默认实现
}
```

### 提案写法

```rust
struct SkillExtension;

impl Extension for SkillExtension {
    fn id(&self) -> &str { "astrcode-skill" }

    fn register(&self, reg: &mut Registrar) {
        reg.tool(
            ToolDefinition { name: "skill".into(), ... },
            Arc::new(SkillToolHandler),
        );
    }
}

struct SkillToolHandler;

#[async_trait]
impl ToolHandler for SkillToolHandler {
    async fn execute(&self, args: Value, ctx: &ToolExecutionContext)
        -> Result<ToolResult, ExtensionError>
    {
        // ... 执行逻辑（和之前一样）
    }
}
```

### 安全审查扩展

**当前**：
```rust
impl Extension for SecurityExtension {
    fn id(&self) -> &str { "security" }
    fn hook_subscriptions(&self) -> Vec<HookSubscription> {
        vec![HookSubscription { event: PreToolUse, mode: Blocking, priority: 0 }]
    }
    async fn on_event(&self, event: ExtensionEvent, ctx: &dyn ExtensionContext)
        -> Result<HookEffect, ExtensionError>
    {
        if event != PreToolUse { return Ok(HookEffect::Allow); }
        let input = ctx.pre_tool_use_input().unwrap();
        if input.tool_name == "shell" && /* rm -rf */ {
            Ok(HookEffect::Block { reason: "dangerous".into() })
        } else {
            Ok(HookEffect::Allow)
        }
    }
}
```

**提案**：
```rust
impl Extension for SecurityExtension {
    fn id(&self) -> &str { "security" }

    fn register(&self, reg: &mut Registrar) {
        reg.on_pre_tool_use(HookMode::Blocking, Arc::new(SecurityGuard));
    }
}

struct SecurityGuard;

#[async_trait]
impl PreToolUseHandler for SecurityGuard {
    async fn handle(&self, ctx: PreToolUseContext)
        -> Result<PreToolUseResult, ExtensionError>
    {
        if ctx.tool_name == "shell" && /* rm -rf */ {
            Ok(PreToolUseResult::Block { reason: "dangerous".into() })
        } else {
            Ok(PreToolUseResult::Allow)
        }
    }
}
```

不需要手动过滤事件。不需要 `pre_tool_use_input()` 方法。返回类型自动约束为 `Allow / Block / ModifyInput`。

### Provider 消息注入扩展

**当前**：需要在 `on_event` 里匹配 `BeforeProviderRequest`，通过 `ctx.provider_messages()` 读取消息，返回 `HookEffect::AppendMessages`。

**提案**：
```rust
impl Extension for PromptInjector {
    fn id(&self) -> &str { "prompt-injector" }

    fn register(&self, reg: &mut Registrar) {
        reg.on_provider(ProviderEvent::BeforeRequest, HookMode::Blocking,
                        Arc::new(InjectHandler));
    }
}

struct InjectHandler;

#[async_trait]
impl ProviderHandler for InjectHandler {
    async fn handle(&self, ctx: ProviderContext)
        -> Result<ProviderResult, ExtensionError>
    {
        Ok(ProviderResult::AppendMessages {
            messages: vec![LlmMessage::system("Current mode: readonly")],
        })
    }
}
```

`ProviderContext` 是 owned struct，包含 `messages: Vec<LlmMessage>`。宿主读取 `ProviderResult` 并应用 patch。**不需要 ProviderMessagesContext 包装器。**

---

## 4. ExtensionRunner 分发

```rust
impl ExtensionRunner {
    /// 通用生命周期事件分发
    pub async fn emit(&self, event: ExtensionEvent, ctx: LifecycleContext)
        -> Result<HookResult, ExtensionError>
    {
        for (mode, handler) in self.handlers_for(&event) {
            match mode {
                HookMode::Blocking => {
                    let result = self.call(handler.as_ref(), ctx.clone()).await?;
                    if let HookResult::Block { reason } = result {
                        return Ok(HookResult::Block { reason });
                    }
                },
                HookMode::NonBlocking => {
                    let ctx = ctx.clone();
                    tokio::spawn(async move {
                        let _ = handler.handle(ctx).await;
                    });
                },
                HookMode::Advisory => {
                    let _ = handler.handle(ctx.clone()).await;
                },
            }
        }
        Ok(HookResult::Allow)
    }

    /// 工具 hook 分发
    pub async fn emit_pre_tool_use(&self, ctx: PreToolUseContext)
        -> Result<PreToolUseResult, ExtensionError>
    { /* 只匹配 PreToolUseResult */ }

    pub async fn emit_post_tool_use(&self, ctx: PostToolUseContext)
        -> Result<PostToolUseResult, ExtensionError>
    { /* 只匹配 PostToolUseResult */ }

    /// Provider hook 分发
    pub async fn emit_provider(&self, event: ProviderEvent, ctx: ProviderContext)
        -> Result<ProviderResult, ExtensionError>
    {
        for (mode, handler) in self.provider_handlers_for(&event) {
            match mode {
                HookMode::Blocking => {
                    let result = handler.handle(ctx.clone()).await?;
                    match result {
                        ProviderResult::Block { reason } =>
                            return Ok(ProviderResult::Block { reason }),
                        ProviderResult::ReplaceMessages { messages } => {
                            // 应用 patch：后续 handler 看到替换后的消息
                            ctx = ProviderContext { messages, ..ctx };
                        },
                        ProviderResult::AppendMessages { messages } => {
                            let mut new_messages = ctx.messages;
                            new_messages.extend(messages);
                            ctx = ProviderContext { messages: new_messages, ..ctx };
                        },
                        ProviderResult::Allow => {},
                    }
                },
                // NonBlocking / Advisory 同理
                _ => {},
            }
        }
        Ok(ProviderResult::Allow)
    }

    /// Prompt build 收集
    pub async fn collect_prompt_contributions(&self, ctx: PromptBuildContext)
        -> Result<PromptContributions, ExtensionError>
    { /* 收集所有 handler 的贡献 */ }

    /// Compact hook 分发
    pub async fn emit_compact(&self, event: CompactEvent, ctx: CompactContext)
        -> Result<CompactResult, ExtensionError>
    { /* 只匹配 CompactResult */ }
}
```

**ProviderMessagesContext 完全消失。** Context 是 owned struct，每次 handler 修改后创建新的 ctx 传给下一个 handler（链式传递）。

---

## 5. FFI 层适配

```rust
impl Extension for NativeExtension {
    fn id(&self) -> &str { &self.id }

    fn register(&self, reg: &mut Registrar) {
        for (event, mode, callback) in self.take_handlers() {
            match event {
                ExtensionEvent::PreToolUse =>
                    reg.on_pre_tool_use(mode, Arc::new(FfiPreToolHandler { callback })),
                ExtensionEvent::PostToolUse =>
                    reg.on_post_tool_use(mode, Arc::new(FfiPostToolHandler { callback })),
                // ... 其他事件
                _ =>
                    reg.on_event(event, mode, Arc::new(FfiLifecycleHandler { callback })),
            }
        }
        for (def, callback) in self.take_tools() {
            reg.tool(def, Arc::new(FfiToolHandler { callback }));
        }
        for (cmd, callback) in self.take_commands() {
            reg.command(cmd, Arc::new(FfiCommandHandler { callback }));
        }
    }
}
```

FFI vtable 不变。只是交接方式从 trait 方法实现变成 `register()` 里的 registrar 调用。

---

## 6. 对比

| 维度 | 当前 | 提案 |
|------|------|------|
| Extension trait | 10 方法 | 2 方法（`id` + `register`） |
| Handler 入口 | 1 个 `on_event()` 处理所有事件 | 按需注册，每个 handler 1 个方法 |
| 返回类型 | 1 个 `HookEffect`（9 变体） | 5 个类型化枚举（各 2-4 变体） |
| Context | 1 个 trait（23 方法） | 6 个 plain struct（各有 4-8 字段） |
| ProviderMessagesContext | 170 行委托样板 | 0 行（不需要） |
| 事件-效果约束 | 运行时 warn | 编译期 |
| 序列化 | 不可能（trait object） | 天然支持（plain struct） |
| Wasm / IPC 准备 | 无 | 有（struct + enum 都是值类型） |
| 新增事件 | 改 Extension trait | 新 handler trait + registrar 方法 |

---

## 7. 新类型清单

| 类别 | 类型 | 说明 |
|------|------|------|
| Trait | `Extension` | 2 方法：id + register |
| Trait | `PreToolUseHandler` | 1 方法 |
| Trait | `PostToolUseHandler` | 1 方法 |
| Trait | `ProviderHandler` | 1 方法 |
| Trait | `PromptBuildHandler` | 1 方法 |
| Trait | `CompactHandler` | 1 方法 |
| Trait | `LifecycleHandler` | 1 方法 |
| Trait | `ToolHandler` | 1 方法 |
| Trait | `CommandHandler` | 1 方法 |
| Struct | `Registrar` | 注册入口 |
| Struct | `PreToolUseContext` | plain struct |
| Struct | `PostToolUseContext` | plain struct |
| Struct | `ProviderContext` | plain struct |
| Struct | `PromptBuildContext` | plain struct |
| Struct | `CompactContext` | plain struct |
| Struct | `LifecycleContext` | plain struct |
| Struct | `CommandContext` | plain struct |
| Enum | `HookResult` | Allow / Block |
| Enum | `PreToolUseResult` | Allow / Block / ModifyInput |
| Enum | `PostToolUseResult` | Allow / Block / ModifyResult |
| Enum | `ProviderResult` | Allow / Block / ReplaceMessages / AppendMessages |
| Enum | `CompactResult` | Allow / Block / Contributions |
| Enum | `ProviderEvent` | BeforeRequest / AfterResponse |
| Enum | `CompactEvent` | PreCompact / PostCompact |

共 9 个 trait + 8 个 context struct + 6 个 result enum + 2 个 event enum + 1 个 registrar struct = **26 个新类型**。

看起来多，但每个类型都极简（1 个方法的 trait，4-8 字段的 struct，2-4 变体的 enum）。
对比当前系统的 1 个 23 方法 trait + 1 个 10 方法 trait + 1 个 9 变体 enum，**认知分布更均匀，单个类型的理解成本更低**。

---

## 8. 权衡

### 收益

- Extension 2 方法 → 扩展作者只需理解 `register()`
- Context plain struct → 0 委托样板，天然可序列化
- 类型化 result → 编译期约束，消除 ignore 分支
- ProviderMessagesContext 消失
- 进程隔离 / Wasm 路径已打通

### 代价

- 9 个 handler trait → 比当前 1 个 Extension trait 的类型多
- `Arc<dyn>` 包装 → 每个 handler 一次堆分配（可忽略）
- 迁移工作量 → 5 个内置扩展 + FFI + Runner 全部重写
- Context struct 需要克隆 → NonBlocking 场景下 clone 成本（纯数据，可接受）

---

## 9. 待决策点

### Q1：是否接受 `register(&mut Registrar)` 模式？

接受意味着引入 handler trait 体系。拒绝意味着保持当前 Extension trait 但做小改良（瘦身 ExtensionContext、类型化 HookEffect）。

### Q2：通用生命周期事件怎么处理？

当前有 14 个 ExtensionEvent 变体，其中大部分是通用的（SessionStart, TurnEnd 等）。

选项：
- **A**：统一用 `LifecycleHandler` + `LifecycleContext` 处理所有通用事件
- **B**：每个事件独立 handler（handler 数量爆炸到 14+）
- **推荐**：A。通用事件只需要 Allow/Block，用一个 handler 类型足够。

### Q3：Priority 怎么处理？

当前系统支持每个扩展在每个事件上有不同的优先级。提案中 `reg.on_pre_tool_use(mode, handler)` 没有传 priority。

选项：
- **A**：priority 放在 registrar 方法参数上 `reg.on_pre_tool_use(mode, priority, handler)`
- **B**：priority 放在 handler trait 方法上
- **推荐**：A。保持和当前行为一致。

### Q4：动态工具发现（`tools_for`）怎么处理？

当前 `tools_for(working_dir)` 支持按项目动态发现工具（MCP 场景）。

选项：
- **A**：registrar 增加 `reg.on_tool_discovery(handler)`
- **B**：ToolHandler trait 加 `fn definitions_for(&self, working_dir: &str) -> Vec<ToolDefinition>`
- **推荐**：A。发现是查询式操作，适合 handler 模式。

### Q5：事件总线保留吗？

保留。但限制为通知用途：
- 通知，不用于请求-响应
- 不能修改当前主流程
- 监听默认拿 snapshot
- topic 必须 namespaced（如 `"mode.changed"`）
- 插件卸载时自动取消订阅

### Q6：迁移策略？

- **A**：双轨运行（新旧都支持）
- **B**：一次性切换
- **推荐**：B。内置扩展只有 5 个。双轨的适配代码比一次性切换还多。

---

## 10. 实施路径

### Phase 1：In-process + 类型化 API（当前阶段）

**目标**：简化扩展编写，消除样板代码，建立正确的抽象。

步骤：
1. 定义新 handler traits + context structs + result enums
2. 定义 Registrar struct
3. ExtensionRunner 内部实现 ExtensionRecord + 新的分发方法
4. 一次性迁移 5 个内置扩展 + FFI
5. 移除旧代码（ProviderMessagesContext, HookEffect, 旧 Extension trait 方法）
6. 全量测试通过

### Phase 2：进程隔离（全能 agent 阶段）

**目标**：第三方扩展安全运行。

步骤：
1. 定义 IPC 协议（基于 registrar 方法签名的 JSON/msgpack 映射）
2. 实现 `IpcHandler` 系列（handler trait 的 IPC stub）
3. 实现 ExtensionProcessManager（启动、心跳、崩溃重启）
4. 内置扩展仍 in-process（可信）
5. 第三方扩展走 IPC（不可信）
6. Context 序列化传输（已天然支持——plain struct）

**Phase 1 的代码在 Phase 2 不需要改**——Extension trait 不变，handler trait 不变，context struct 不变，只是 handler 的内部实现从直接调用换成 IPC。

### 未来（不做但不堵死）

- **Wasm 沙箱**：handler trait 实现为 Wasm 调用，context struct 映射到 WIT 类型
- **多语言 SDK**：IPC 协议语言无关，Python/JS 可实现 client
- **远程扩展**：IPC 从本地进程换成网络传输

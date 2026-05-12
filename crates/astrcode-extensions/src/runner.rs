//! 扩展运行器 — 将生命周期事件分发到已注册的扩展。
//!
//! 负责管理扩展注册、事件分发，并强制执行 HookMode 语义：
//! - Blocking: 同步执行，可返回 Block 或 ModifiedInput/ModifiedResult
//! - NonBlocking: 以即发即弃方式派生任务，使用快照上下文
//! - Advisory: 结果仅记录日志，不强制执行

use std::{
    sync::{Arc, RwLock as StdRwLock},
    time::Duration,
};

use astrcode_core::{
    extension::*,
    tool::{ExecutionMode, Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolResult},
};
use tokio::sync::RwLock;

use crate::runtime::{SessionSpawner, SpawnRequest, SpawnResult};

/// 将生命周期事件分发到所有已注册的扩展。
///
/// 强制执行 HookMode 语义：
/// - Blocking: 同步执行，可返回 Block 或 ModifiedInput/ModifiedResult
/// - NonBlocking: 以即发即弃方式派生任务，使用快照上下文
/// - Advisory: 结果仅记录日志，不强制执行
pub struct ExtensionRunner {
    /// 已注册的扩展列表（读写锁保护）
    extensions: RwLock<Vec<Arc<dyn Extension>>>,
    /// 从 register() 收集的类型化能力记录
    records: RwLock<Vec<ExtensionRecord>>,
    /// 会话创建器（在 bind() 调用前为 None）
    spawner: Arc<StdRwLock<Option<Arc<dyn SessionSpawner>>>>,
    /// 钩子执行超时时间
    timeout: Duration,
}

/// 从 `register()` 调用中收集的扩展能力记录。
struct ExtensionRecord {
    id: String,
    reg: Registrar,
}

#[derive(Debug, Clone)]
pub struct RegisteredSlashCommand {
    pub extension_id: String,
    pub command: astrcode_core::extension::SlashCommand,
}

impl ExtensionRunner {
    /// 创建新的扩展运行器。
    ///
    /// # 参数
    /// - `timeout`: 阻塞钩子的执行超时时间
    pub fn new(timeout: Duration) -> Self {
        Self {
            extensions: RwLock::new(Vec::new()),
            records: RwLock::new(Vec::new()),
            spawner: Arc::new(StdRwLock::new(None)),
            timeout,
        }
    }

    /// 注册一个扩展。
    ///
    /// 调用 `ext.register()` 收集类型化能力，然后存入扩展列表。
    /// 重复的扩展 ID 会被跳过并记录警告。
    pub async fn register(&self, ext: Arc<dyn Extension>) {
        let id = ext.id().to_string();

        {
            let exts = self.extensions.read().await;
            if exts.iter().any(|e| e.id() == id) {
                tracing::warn!(extension_id = %id, "extension already registered, skipping duplicate");
                return;
            }
        }

        let mut reg = Registrar::new();
        ext.register(&mut reg);
        if !reg.is_empty() {
            let mut records = self.records.write().await;
            records.push(ExtensionRecord {
                id: id.clone(),
                reg,
            });
        }
        let mut exts = self.extensions.write().await;
        exts.push(ext);
    }

    /// 绑定会话创建能力。
    /// 在服务器启动后、任何工具执行之前调用一次。
    pub fn bind(&self, spawner: Arc<dyn SessionSpawner>) {
        *self.spawner.write().unwrap_or_else(|e| e.into_inner()) = Some(spawner);
    }

    pub async fn count(&self) -> usize {
        self.extensions.read().await.len()
    }

    // ─── 类型化分发方法（Context + Handler） ──────────────────

    /// PreToolUse 钩子分发。
    pub async fn emit_pre_tool_use(
        &self,
        ctx: PreToolUseContext,
    ) -> Result<PreToolUseResult, ExtensionError> {
        let records = self.records.read().await;
        let mut handlers: Vec<(i32, HookMode, Arc<dyn PreToolUseHandler>)> = Vec::new();
        for record in records.iter() {
            for (mode, priority, handler) in record.reg.pre_tool_use.iter() {
                handlers.push((*priority, *mode, Arc::clone(handler)));
            }
        }
        handlers.sort_by_key(|b| std::cmp::Reverse(b.0));
        drop(records);

        let mut ctx = ctx;
        for (_, mode, handler) in handlers {
            match mode {
                HookMode::Blocking => {
                    let result = tokio::time::timeout(self.timeout, handler.handle(ctx.clone()))
                        .await
                        .map_err(|_| ExtensionError::Timeout(self.timeout.as_millis() as u64))??;
                    match result {
                        PreToolUseResult::Block { reason } => {
                            return Ok(PreToolUseResult::Block { reason });
                        },
                        PreToolUseResult::ModifyInput { tool_input } => {
                            ctx = PreToolUseContext { tool_input, ..ctx };
                        },
                        PreToolUseResult::Allow => {},
                    }
                },
                HookMode::Advisory => {
                    if let Err(e) = handler.handle(ctx.clone()).await {
                        tracing::warn!(extension_event = "pre_tool_use", error = %e, "advisory handler failed");
                    }
                },
                HookMode::NonBlocking => {
                    let ctx = ctx.clone();
                    spawn_nonblocking(async move {
                        if let Err(e) = handler.handle(ctx).await {
                            tracing::warn!(extension_event = "pre_tool_use", error = %e, "non-blocking handler failed");
                        }
                    });
                },
            }
        }
        Ok(PreToolUseResult::Allow)
    }

    /// PostToolUse 钩子分发。
    pub async fn emit_post_tool_use(
        &self,
        ctx: PostToolUseContext,
    ) -> Result<PostToolUseResult, ExtensionError> {
        let records = self.records.read().await;
        let mut handlers: Vec<(i32, HookMode, Arc<dyn PostToolUseHandler>)> = Vec::new();
        for record in records.iter() {
            for (mode, priority, handler) in record.reg.post_tool_use.iter() {
                handlers.push((*priority, *mode, Arc::clone(handler)));
            }
        }
        handlers.sort_by_key(|b| std::cmp::Reverse(b.0));
        drop(records);

        let mut ctx = ctx;
        for (_, mode, handler) in handlers {
            match mode {
                HookMode::Blocking => {
                    let result = tokio::time::timeout(self.timeout, handler.handle(ctx.clone()))
                        .await
                        .map_err(|_| ExtensionError::Timeout(self.timeout.as_millis() as u64))??;
                    match result {
                        PostToolUseResult::Block { reason } => {
                            return Ok(PostToolUseResult::Block { reason });
                        },
                        PostToolUseResult::ModifyResult { content } => {
                            ctx = PostToolUseContext {
                                tool_result: ToolResult {
                                    content,
                                    ..ctx.tool_result
                                },
                                ..ctx
                            };
                        },
                        PostToolUseResult::Allow => {},
                    }
                },
                HookMode::Advisory => {
                    if let Err(e) = handler.handle(ctx.clone()).await {
                        tracing::warn!(extension_event = "post_tool_use", error = %e, "advisory handler failed");
                    }
                },
                HookMode::NonBlocking => {
                    let ctx = ctx.clone();
                    spawn_nonblocking(async move {
                        if let Err(e) = handler.handle(ctx).await {
                            tracing::warn!(extension_event = "post_tool_use", error = %e, "non-blocking handler failed");
                        }
                    });
                },
            }
        }
        Ok(PostToolUseResult::Allow)
    }

    /// Provider 钩子分发。
    pub async fn emit_provider(
        &self,
        event: ProviderEvent,
        ctx: ProviderContext,
    ) -> Result<ProviderResult, ExtensionError> {
        let records = self.records.read().await;
        let mut handlers: Vec<(i32, HookMode, Arc<dyn ProviderHandler>)> = Vec::new();
        for record in records.iter() {
            for (ev, mode, priority, handler) in record.reg.provider.iter() {
                if *ev == event {
                    handlers.push((*priority, *mode, Arc::clone(handler)));
                }
            }
        }
        handlers.sort_by_key(|b| std::cmp::Reverse(b.0));
        drop(records);

        let mut ctx = ctx;
        let mut modified = false;
        for (_, mode, handler) in handlers {
            match mode {
                HookMode::Blocking => {
                    let result = tokio::time::timeout(self.timeout, handler.handle(ctx.clone()))
                        .await
                        .map_err(|_| ExtensionError::Timeout(self.timeout.as_millis() as u64))??;
                    match result {
                        ProviderResult::Block { reason } => {
                            return Ok(ProviderResult::Block { reason });
                        },
                        ProviderResult::ReplaceMessages { messages } => {
                            ctx = ProviderContext { messages, ..ctx };
                            modified = true;
                        },
                        ProviderResult::AppendMessages { messages } => {
                            let mut new_messages = ctx.messages;
                            new_messages.extend(messages);
                            ctx = ProviderContext { messages: new_messages, ..ctx };
                            modified = true;
                        },
                        ProviderResult::Allow => {},
                    }
                },
                HookMode::Advisory => {
                    if let Err(e) = handler.handle(ctx.clone()).await {
                        tracing::warn!(extension_event = "provider", error = %e, "advisory handler failed");
                    }
                },
                HookMode::NonBlocking => {
                    let ctx = ctx.clone();
                    spawn_nonblocking(async move {
                        if let Err(e) = handler.handle(ctx).await {
                            tracing::warn!(extension_event = "provider", error = %e, "non-blocking handler failed");
                        }
                    });
                },
            }
        }
        if modified {
            Ok(ProviderResult::ReplaceMessages { messages: ctx.messages })
        } else {
            Ok(ProviderResult::Allow)
        }
    }

    /// PromptBuild 贡献收集（类型化版本）。
    pub async fn collect_prompt_contributions_typed(
        &self,
        ctx: PromptBuildContext,
    ) -> Result<PromptContributions, ExtensionError> {
        let records = self.records.read().await;
        let mut handlers: Vec<(i32, Arc<dyn PromptBuildHandler>)> = Vec::new();
        for record in records.iter() {
            for (priority, handler) in record.reg.prompt_build.iter() {
                handlers.push((*priority, Arc::clone(handler)));
            }
        }
        handlers.sort_by_key(|b| std::cmp::Reverse(b.0));
        drop(records);

        let mut collected = PromptContributions::default();
        for (_, handler) in handlers {
            let contributions = tokio::time::timeout(self.timeout, handler.handle(ctx.clone()))
                .await
                .map_err(|_| ExtensionError::Timeout(self.timeout.as_millis() as u64))??;
            collected.merge(contributions);
        }
        Ok(collected)
    }

    /// Compact 钩子分发。
    pub async fn emit_compact(
        &self,
        event: CompactEvent,
        ctx: CompactContext,
    ) -> Result<CompactResult, ExtensionError> {
        let records = self.records.read().await;
        let mut handlers: Vec<(i32, Arc<dyn CompactHandler>)> = Vec::new();
        for record in records.iter() {
            for (ev, priority, handler) in record.reg.compact.iter() {
                if *ev == event {
                    handlers.push((*priority, Arc::clone(handler)));
                }
            }
        }
        handlers.sort_by_key(|b| std::cmp::Reverse(b.0));
        drop(records);

        let mut collected = CompactContributions::default();
        for (_, handler) in handlers {
            let result = tokio::time::timeout(self.timeout, handler.handle(ctx.clone()))
                .await
                .map_err(|_| ExtensionError::Timeout(self.timeout.as_millis() as u64))??;
            match result {
                CompactResult::Block { reason } => {
                    return Ok(CompactResult::Block { reason });
                },
                CompactResult::Contributions(c) => {
                    collected.merge(c);
                },
                CompactResult::Allow => {},
            }
        }
        if collected.instructions.is_empty() {
            Ok(CompactResult::Allow)
        } else {
            Ok(CompactResult::Contributions(collected))
        }
    }

    /// PostToolUseFailure 通知型钩子分发。
    pub async fn emit_post_tool_use_failure(
        &self,
        ctx: PostToolUseFailureContext,
    ) {
        let records = self.records.read().await;
        let mut handlers: Vec<(i32, Arc<dyn PostToolUseFailureHandler>)> = Vec::new();
        for record in records.iter() {
            for (priority, handler) in record.reg.post_tool_use_failure.iter() {
                handlers.push((*priority, Arc::clone(handler)));
            }
        }
        handlers.sort_by_key(|b| std::cmp::Reverse(b.0));
        drop(records);

        for (_, handler) in handlers {
            match tokio::time::timeout(self.timeout, handler.handle(ctx.clone())).await {
                Ok(Ok(())) => {},
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "post tool use failure handler failed");
                },
                Err(_) => {
                    tracing::warn!("post tool use failure handler timed out");
                },
            }
        }
    }

    /// 通用生命周期事件分发。
    pub async fn emit_lifecycle(
        &self,
        event: ExtensionEvent,
        ctx: LifecycleContext,
    ) -> Result<HookResult, ExtensionError> {
        let records = self.records.read().await;
        let mut handlers: Vec<(i32, HookMode, Arc<dyn LifecycleHandler>)> = Vec::new();
        for record in records.iter() {
            for (ev, mode, priority, handler) in record.reg.lifecycle.iter() {
                if *ev == event {
                    handlers.push((*priority, *mode, Arc::clone(handler)));
                }
            }
        }
        handlers.sort_by_key(|b| std::cmp::Reverse(b.0));
        drop(records);

        for (_, mode, handler) in handlers {
            match mode {
                HookMode::Blocking => {
                    let result = tokio::time::timeout(self.timeout, handler.handle(ctx.clone()))
                        .await
                        .map_err(|_| ExtensionError::Timeout(self.timeout.as_millis() as u64))??;
                    if let HookResult::Block { reason } = result {
                        return Ok(HookResult::Block { reason });
                    }
                },
                HookMode::Advisory => {
                    if let Err(e) = handler.handle(ctx.clone()).await {
                        tracing::warn!(extension_event = "lifecycle", error = %e, "advisory handler failed");
                    }
                },
                HookMode::NonBlocking => {
                    let ctx = ctx.clone();
                    spawn_nonblocking(async move {
                        if let Err(e) = handler.handle(ctx).await {
                            tracing::warn!(extension_event = "lifecycle", error = %e, "non-blocking handler failed");
                        }
                    });
                },
            }
        }
        Ok(HookResult::Allow)
    }

    /// 从 ExtensionRecord 收集工具适配器（类型化版本）。
    pub async fn collect_tool_adapters_typed(&self, working_dir: &str) -> Vec<Arc<dyn Tool>> {
        let records = self.records.read().await;
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
        for record in records.iter() {
            for (def, handler) in record.reg.tools.iter() {
                tools.push(Arc::new(HandlerTool {
                    definition: def.clone(),
                    handler: Arc::clone(handler),
                    working_dir: working_dir.to_string(),
                    spawner: Arc::clone(&self.spawner),
                }));
            }
            for discovery in record.reg.tool_discovery.iter() {
                match tokio::time::timeout(self.timeout, discovery.discover(working_dir)).await {
                    Ok(discovered) => {
                        for (def, handler) in discovered {
                            tools.push(Arc::new(HandlerTool {
                                definition: def,
                                handler,
                                working_dir: working_dir.to_string(),
                                spawner: Arc::clone(&self.spawner),
                            }));
                        }
                    },
                    Err(_) => {
                        tracing::warn!("tool discovery timed out for extension {}", record.id);
                    },
                }
            }
        }
        tools
    }

    /// 从 ExtensionRecord 收集工具提示词元数据（类型化版本）。
    pub async fn collect_tool_prompt_metadata_typed(
        &self,
    ) -> std::collections::HashMap<String, astrcode_core::tool::ToolPromptMetadata> {
        let records = self.records.read().await;
        let mut map = std::collections::HashMap::new();
        for record in records.iter() {
            map.extend(record.reg.tool_metadata.clone());
        }
        map
    }

    /// 从 ExtensionRecord 收集斜杠命令（类型化版本）。
    pub async fn collect_commands_for_typed(
        &self,
        working_dir: &str,
    ) -> Vec<(String, SlashCommand, Arc<dyn CommandHandler>)> {
        let records = self.records.read().await;
        let mut cmds = Vec::new();
        for record in records.iter() {
            for (cmd, handler) in record.reg.commands.iter() {
                cmds.push((record.id.clone(), cmd.clone(), Arc::clone(handler)));
            }
            for discovery in record.reg.command_discovery.iter() {
                match tokio::time::timeout(self.timeout, discovery.discover(working_dir)).await {
                    Ok(discovered) => {
                        for (cmd, handler) in discovered {
                            cmds.push((record.id.clone(), cmd, handler));
                        }
                    },
                    Err(_) => {
                        tracing::warn!("command discovery timed out for extension {}", record.id);
                    },
                }
            }
        }
        cmds
    }

    /// 命令派发（类型化版本）。
    pub async fn dispatch_command_typed(
        &self,
        command_name: &str,
        arguments: &str,
        working_dir: &str,
        ctx: &CommandContext,
    ) -> Result<ExtensionCommandResult, ExtensionError> {
        let cmds = self.collect_commands_for_typed(working_dir).await;
        let mut matched: Vec<(String, SlashCommand, Arc<dyn CommandHandler>)> = cmds
            .into_iter()
            .filter(|(_, cmd, _)| cmd.name == command_name)
            .collect();
        matched.sort_by_key(|a| command_dispatch_priority(&a.0));

        if let Some((_, _, handler)) = matched.into_iter().next() {
            handler
                .execute(command_name, arguments, working_dir, ctx)
                .await
        } else {
            Err(ExtensionError::NotFound(command_name.into()))
        }
    }

    /// 判断是否有任何扩展使用了 register() 注册了类型化能力。
    pub async fn has_records(&self) -> bool {
        !self.records.read().await.is_empty()
    }
}

fn command_dispatch_priority(extension_id: &str) -> u8 {
    if extension_id == "astrcode-skill" {
        1
    } else {
        0
    }
}

/// 以即发即弃方式派生异步任务，观察 panic 并记录错误日志。
///
/// `tokio::spawn` 的 JoinHandle 被丢弃时，任务内的 panic 会被静默吞掉。
/// 此函数通过第二个轻量级任务观察原始任务的 JoinHandle，
/// 确保 panic 至少以 error 级别被记录。
fn spawn_nonblocking<F>(fut: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let handle = tokio::spawn(fut);
    tokio::spawn(async move {
        if let Err(e) = handle.await {
            if e.is_panic() {
                tracing::error!("non-blocking handler panicked");
            }
        }
    });
}

/// 类型化工具适配器，将 `ToolHandler` 包装为 `Tool` trait 实现。
struct HandlerTool {
    definition: ToolDefinition,
    handler: Arc<dyn ToolHandler>,
    working_dir: String,
    spawner: Arc<StdRwLock<Option<Arc<dyn SessionSpawner>>>>,
}

impl HandlerTool {
    async fn spawn(
        &self,
        parent_session_id: &str,
        request: SpawnRequest,
    ) -> Result<SpawnResult, String> {
        let spawner = {
            let guard = self.spawner.read().unwrap_or_else(|e| e.into_inner());
            match &*guard {
                Some(s) => Arc::clone(s),
                None => return Err("Session spawner not bound".into()),
            }
        };
        spawner.spawn(parent_session_id, request).await
    }
}

#[async_trait::async_trait]
impl Tool for HandlerTool {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    fn execution_mode(&self) -> ExecutionMode {
        self.definition.execution_mode
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        _ctx: &ToolExecutionContext,
    ) -> Result<ToolResult, ToolError> {
        let mut result = match self
            .handler
            .execute(&self.definition.name, arguments, &self.working_dir, _ctx)
            .await
        {
            Ok(result) => result,
            Err(err) => {
                return Ok(extension_error_result(
                    &self.definition.name,
                    "handler",
                    err,
                ));
            },
        };

        if let Some(outcome_value) = result.metadata.remove("extension_tool_outcome") {
            if let Ok(ExtensionToolOutcome::RunSession {
                name,
                system_prompt,
                user_prompt,
                model_preference,
                wait_for_result,
            }) = serde_json::from_value(outcome_value)
            {
                let request = SpawnRequest {
                    name,
                    system_prompt,
                    user_prompt,
                    working_dir: _ctx.working_dir.clone(),
                    model_preference,
                    tool_call_id: _ctx.tool_call_id.clone(),
                    event_tx: _ctx.event_tx.clone(),
                    wait_for_result,
                };

                match self.spawn(_ctx.session_id.as_str(), request).await {
                    Ok(output) => {
                        result.content = output.content;
                        result
                            .metadata
                            .insert("child_session_id".into(), output.child_session_id.into());
                        if let Some(task_id) = output.background_task_id {
                            result
                                .metadata
                                .insert("backgrounded".into(), serde_json::json!(true));
                            result
                                .metadata
                                .insert("task_id".into(), serde_json::json!(task_id));
                        }
                    },
                    Err(e) => {
                        result.content = format!("Failed to spawn child session: {e}");
                        result.is_error = true;
                        result.error = Some(e);
                    },
                }
            }
        }

        Ok(result)
    }
}

/// 将 [`ExtensionError`] 转换为结构化的错误 [`ToolResult`]，供 agent 理解和恢复。
///
/// 与 `ToolError`（纯字符串）不同，`ToolResult` 携带 metadata，
/// agent 可以据此判断是重试、换工具还是报告给用户。
fn extension_error_result(tool_name: &str, extension_id: &str, err: ExtensionError) -> ToolResult {
    use astrcode_core::tool::tool_metadata;

    let (content, suggestion) = match &err {
        ExtensionError::NotFound(_) => (
            format!("Tool `{tool_name}` is not available."),
            "This tool may have been unregistered. Try `tool_search_tool` to discover available \
             tools, or proceed without it.",
        ),
        ExtensionError::Timeout(ms) => (
            format!("Tool `{tool_name}` timed out after {ms}ms."),
            "The extension is still processing. Try again with a simpler request, or proceed \
             without this tool.",
        ),
        ExtensionError::Blocked { reason } => (
            format!("Tool `{tool_name}` was blocked: {reason}"),
            "A hook policy prevented this operation. Check the reason above and adjust your \
             approach.",
        ),
        ExtensionError::Internal(message) => (
            format!("Tool `{tool_name}` failed: {message}"),
            "The extension encountered an internal error. Try again with different arguments, or \
             use a builtin tool as an alternative.",
        ),
    };

    let mut metadata = tool_metadata([
        ("extensionId", serde_json::json!(extension_id)),
        ("toolName", serde_json::json!(tool_name)),
        ("suggestion", serde_json::json!(suggestion)),
    ]);
    if let ExtensionError::Timeout(ms) = &err {
        metadata.insert("timeoutMs".into(), serde_json::json!(ms));
    }

    ToolResult::text(content, true, metadata)
}

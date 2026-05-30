//! 工具调用执行实现。
//!
//! 包含阻塞式执行、带后台化能力的执行。

use std::{sync::Arc, time::Instant};

use astrcode_core::{
    event::EventPayload,
    storage::ToolResultArtifactReader,
    tool::{
        BackgroundPolicy, BackgroundTaskReader, FileObservation, FileObservationStore, LlmModelIds,
        ToolCapabilities, ToolDefinition, ToolError, ToolExecutionContext, ToolResult,
    },
    types::*,
};
use astrcode_tools::registry::ToolRegistry;
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use super::{
    background::{BackgroundTasks, backgrounded_placeholder_result},
    deferred_tools::suggest_tool_alias,
    session::Session,
    tool_types::ExecutableToolCall,
    turn_publish::TurnEvents,
};

// ─── Runtime context types ──────────────────────────────────────────────

/// Turn 级工具上下文：hook 共享字段 + session 基础设施能力。
#[derive(Clone)]
pub(crate) struct TurnToolContext {
    pub shared: crate::turn_context::SharedTurnContext,
    pub capabilities: ToolRuntimeCapabilities,
}

impl TurnToolContext {
    pub(crate) fn for_turn(
        session: &Session,
        session_state: &astrcode_core::storage::SessionReadModel,
        session_store_dir: Option<std::path::PathBuf>,
    ) -> Self {
        let shared = crate::turn_context::SharedTurnContext {
            session_id: session.id().clone(),
            working_dir: session_state.working_dir.clone(),
            model_id: session_state.model_id.clone(),
            session_store_dir: session_store_dir.clone(),
            turn_event_tx: None,
        };
        let capabilities = ToolRuntimeCapabilities::from_session(session, &shared);
        Self {
            shared,
            capabilities,
        }
    }
}

/// 会话级工具运行时能力，从 [`TurnToolContext`] 透传到 [`ToolExecutionContext`]。
#[derive(Clone)]
pub(crate) struct ToolRuntimeCapabilities {
    /// 后台任务管理器，用于注册 handle 以支持取消。
    pub background_tasks: Arc<parking_lot::Mutex<BackgroundTasks>>,
    /// 后台任务只读接口，注入到 ToolExecutionContext 供 TaskTool 使用。
    pub background_task_reader: Option<Arc<dyn BackgroundTaskReader>>,
    /// 文件观察存储，用于 read/edit 协作的 read-before-edit 守卫。
    pub file_observation_store: Option<Arc<dyn FileObservationStore>>,
    /// 会话原子操作能力，供 agent 工具使用。
    pub session_ops: Option<Arc<dyn astrcode_core::tool::SessionOperations>>,
    /// 主模型 ID，供声明 `main_model` 的插件使用。
    pub main_model_id: Option<String>,
    /// 小模型 ID，供子 agent / 声明 `small_model` 的插件使用。
    pub small_model_id: Option<String>,
    /// 分档模型 id（注入 ToolCapabilities 前由 runner 按能力裁剪）。
    pub llm_models: LlmModelIds,
    /// session 在存储层的真实目录路径。
    pub session_store_dir: Option<std::path::PathBuf>,
}

impl ToolRuntimeCapabilities {
    fn from_session(session: &Session, shared: &crate::turn_context::SharedTurnContext) -> Self {
        let runtime = Arc::clone(&session.runtime);
        let caps = session.caps();
        let background_task_reader: Option<Arc<dyn BackgroundTaskReader>> =
            Some(Arc::new(crate::background::BackgroundTaskReaderImpl::new(
                runtime.background_tasks(),
                shared.session_store_dir.clone(),
            )));
        let effective = caps.read_effective();
        let main_model_id = shared.model_id.clone();
        let small_model_id = effective.small_llm.model_id.clone();
        Self {
            background_tasks: runtime.background_tasks(),
            background_task_reader,
            file_observation_store: Some(runtime.file_observation_store()),
            session_ops: caps.session_ops(),
            small_model_id: Some(small_model_id.clone()),
            session_store_dir: shared.session_store_dir.clone(),
            main_model_id: Some(main_model_id.clone()),
            llm_models: LlmModelIds {
                main: Some(main_model_id),
                small: Some(small_model_id),
            },
        }
    }
}

pub(crate) struct ToolCallRuntimeContext {
    pub turn: TurnToolContext,
    pub tools: Vec<ToolDefinition>,
    pub tool_result_reader: Option<Arc<dyn ToolResultArtifactReader>>,
    pub publisher: Arc<TurnEvents>,
    pub cancellation_token: CancellationToken,
    pub session: Session,
}

fn error_tool_result(
    call_id: String,
    tool_name: &str,
    err: ToolError,
    duration: std::time::Duration,
) -> ToolResult {
    use astrcode_core::tool::tool_metadata;

    let (message, suggestion): (String, String) = match &err {
        ToolError::NotFound(name) => {
            if let Some(alias) = suggest_tool_alias(name) {
                (
                    format!("Tool `{name}` not found."),
                    format!("Use `{alias}` instead (exact name from the provider tool list)."),
                )
            } else if name.starts_with("mcp__") {
                (
                    format!("Tool `{name}` not found."),
                    "Call `tool_search_tool` first to load the MCP tool schema, then retry with \
                     the exact `mcp__...` name from the search result."
                        .to_string(),
                )
            } else {
                (
                    format!("Tool `{name}` not found."),
                    "Use an exact tool name from the provider tool list. Match file paths with \
                     `glob` (`pattern` arg) and search contents with `grep`. For external MCP \
                     tools, call `tool_search_tool` first."
                        .to_string(),
                )
            }
        },
        ToolError::InvalidArguments(detail) => (
            format!("Invalid arguments for `{tool_name}`: {detail}"),
            "Re-read the parameter schema and retry with corrected arguments. Do not retry with \
             the same arguments."
                .to_string(),
        ),
        ToolError::Execution(detail) => (
            format!("`{tool_name}` failed: {detail}"),
            "Inspect the error above. Adjust arguments or pick a different approach. Do not retry \
             the identical call."
                .to_string(),
        ),
        ToolError::Blocked { reason } => (
            format!("`{tool_name}` was blocked: {reason}"),
            "A hook policy prevented this. Read the reason and adjust your approach instead of \
             retrying."
                .to_string(),
        ),
        ToolError::Timeout(ms) => (
            format!("`{tool_name}` timed out after {ms}ms."),
            "The process may still be running in the background. Use `task` to inspect or cancel \
             it, or retry with a smaller scope."
                .to_string(),
        ),
    };

    // suggestion 拼接进 content,LLM 才能看到——单独放进 metadata 不会进 prompt。
    let llm_visible = format!("{message}\nSuggestion: {suggestion}");

    let mut metadata = tool_metadata([
        ("toolName", serde_json::json!(tool_name)),
        ("suggestion", serde_json::json!(suggestion)),
    ]);
    if let ToolError::Timeout(ms) = &err {
        metadata.insert("timeoutMs".into(), serde_json::json!(ms));
    }

    ToolResult {
        call_id,
        content: llm_visible.clone(),
        is_error: true,
        error: Some(llm_visible),
        metadata,
        duration_ms: Some(duration.as_millis() as u64),
    }
}

/// 工具在执行完成前被中断（取消、abort、协议修复）时的统一错误结果。
pub fn interrupted_tool_result(
    call_id: String,
    tool_name: &str,
    duration: std::time::Duration,
) -> ToolResult {
    error_tool_result(
        call_id,
        tool_name,
        ToolError::Execution("tool execution interrupted before completion".into()),
        duration,
    )
}

/// 执行单个工具调用，并把异常统一转成工具错误结果。
///
/// 当工具声明了 [`BackgroundPolicy::AutoAfter`] 且执行超过阈值时，
/// 自动将任务转入后台执行，并返回一个占位结果让 LLM 继续推理。
///
/// 工具参数中的 `run_in_background` 字段可以覆盖策略：
/// - `true` → 立即后台化（阈值降为 0）
/// - `false` → 禁止自动后台化（视为 `Never`）
/// - 未设置 → 使用工具声明的默认策略
pub async fn execute_tool_call(
    tool_registry: Arc<ToolRegistry>,
    runtime: ToolCallRuntimeContext,
    mut call: ExecutableToolCall,
) -> (usize, ToolResult) {
    if runtime.cancellation_token.is_cancelled() {
        return (
            call.index,
            interrupted_tool_result(call.call_id.clone(), &call.name, std::time::Duration::ZERO),
        );
    }
    let policy = tool_registry.background_policy(&call.name);
    let effective_policy = resolve_effective_policy(policy, &call.tool_input);

    // run_in_background 是执行层元参数，不属于工具本身的入参。
    if let Some(obj) = call.tool_input.as_object_mut() {
        obj.remove("runInBackground");
    }

    match effective_policy {
        BackgroundPolicy::Never => execute_tool_call_blocking(tool_registry, runtime, call).await,
        BackgroundPolicy::AutoAfter { threshold_secs } => {
            execute_tool_call_with_background(tool_registry, runtime, call, threshold_secs).await
        },
    }
}

/// 根据工具声明的策略和每次调用的参数，决定实际的后台化策略。
fn resolve_effective_policy(
    declared: BackgroundPolicy,
    tool_input: &serde_json::Value,
) -> BackgroundPolicy {
    match tool_input.get("runInBackground").and_then(|v| v.as_bool()) {
        // 显式请求后台化：立即转入后台（阈值 0）
        Some(true) => BackgroundPolicy::AutoAfter { threshold_secs: 0 },
        // 显式禁止后台化：视为 Never
        Some(false) => BackgroundPolicy::Never,
        // 未设置：使用工具声明的策略
        None => declared,
    }
}

use crate::turn_publish::spawn_event_bridge;

fn tool_capabilities_from_runtime(runtime: &ToolCallRuntimeContext) -> ToolCapabilities {
    let capabilities = &runtime.turn.capabilities;
    ToolCapabilities {
        model_id: Some(runtime.turn.shared.model_id.clone()),
        main_model_id: capabilities.main_model_id.clone(),
        small_model_id: capabilities.small_model_id.clone(),
        llm_models: capabilities.llm_models.clone(),
        session_store_dir: capabilities.session_store_dir.clone(),
        available_tools: Some(runtime.tools.clone()),
        tool_result_reader: runtime.tool_result_reader.clone(),
        background_task_reader: capabilities.background_task_reader.clone(),
        file_observation_store: capabilities.file_observation_store.clone(),
        session_ops: capabilities.session_ops.clone(),
        extension_event_sink: None,
    }
}

/// 普通的阻塞式工具执行（原有逻辑）。
async fn execute_tool_call_blocking(
    tool_registry: Arc<ToolRegistry>,
    runtime: ToolCallRuntimeContext,
    call: ExecutableToolCall,
) -> (usize, ToolResult) {
    let started_at = Instant::now();
    let tool_name = call.name;
    let call_id = call.call_id.clone();
    let capabilities = tool_capabilities_from_runtime(&runtime);
    let tool_event_bridge = Some(spawn_event_bridge(runtime.publisher));
    let tool_event_tx = tool_event_bridge
        .as_ref()
        .map(|(tool_tx, _)| tool_tx.clone());
    let tool_ctx = ToolExecutionContext {
        session_id: runtime.turn.shared.session_id.clone(),
        working_dir: runtime.turn.shared.working_dir.clone(),
        tool_call_id: Some(call.call_id.clone()),
        event_tx: tool_event_tx,
        capabilities,
    };

    let result = match tokio::select! {
        _ = runtime.cancellation_token.cancelled() => {
            Err(ToolError::Execution("tool execution interrupted before completion".into()))
        },
        result = tool_registry.execute(&tool_name, call.tool_input, &tool_ctx) => result,
    } {
        Ok(mut result) => {
            result.call_id = call.call_id.clone();
            result.duration_ms = Some(started_at.elapsed().as_millis() as u64);
            result
        },
        Err(e) => error_tool_result(call.call_id.clone(), &tool_name, e, started_at.elapsed()),
    };
    // Release the tool-side sender before awaiting the bridge; otherwise the
    // bridge keeps waiting for more tool progress events and this call hangs.
    drop(tool_ctx);
    if let Some((tool_tx, bridge)) = tool_event_bridge {
        drop(tool_tx);
        if let Err(e) = bridge.await {
            tracing::error!(tool_name, call_id, panic = %e, "event bridge task panicked");
        }
    }

    if result.is_error {
        tracing::warn!(
            tool_name,
            call_id,
            duration_ms = result.duration_ms.unwrap_or_default(),
            error = result.error.as_deref().unwrap_or("unknown error"),
            "tool execution completed with error"
        );
    } else {
        tracing::debug!(
            tool_name,
            call_id,
            duration_ms = result.duration_ms.unwrap_or_default(),
            "tool execution completed"
        );
    }

    (call.index, result)
}

/// 带后台化能力的工具执行。
///
/// spawn 单个 async task 完成执行 → 写磁盘 → 发事件 → 从 manager 移除的全生命周期。
/// 超时则保留 task 继续在后台执行，返回占位结果。
async fn execute_tool_call_with_background(
    tool_registry: Arc<ToolRegistry>,
    runtime: ToolCallRuntimeContext,
    call: ExecutableToolCall,
    threshold_secs: u64,
) -> (usize, ToolResult) {
    let started_at = Instant::now();
    let tool_name = call.name.clone();
    let call_id = call.call_id.clone();
    let call_index = call.index;

    let tool_event_bridge = spawn_event_bridge(Arc::clone(&runtime.publisher));
    let tool_event_tx = Some(tool_event_bridge.0.clone());

    let tool_ctx = ToolExecutionContext {
        session_id: runtime.turn.shared.session_id.clone(),
        working_dir: runtime.turn.shared.working_dir.clone(),
        tool_call_id: Some(call.call_id.clone()),
        event_tx: tool_event_tx,
        capabilities: tool_capabilities_from_runtime(&runtime),
    };

    // 共享结果槽：task 写入，主线程读取（仅用于快速路径）
    let result_slot = Arc::new(Mutex::new(
        None::<Result<ToolResult, astrcode_core::tool::ToolError>>,
    ));
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
    let (backgrounded_tx, backgrounded_rx) = tokio::sync::oneshot::channel::<()>();

    let name = call.name.clone();
    let tool_input = call.tool_input.clone();
    let slot_writer = Arc::clone(&result_slot);

    // 准备后台路径所需的变量
    let bg_task_id = new_background_task_id();
    let bg_task_id_for_cancel = bg_task_id.clone();
    let bg_task_id_for_closure = bg_task_id.clone();
    let bg_session = runtime.session.clone();
    let bg_manager = runtime.turn.capabilities.background_tasks.clone();
    let bg_store_dir = runtime.turn.capabilities.session_store_dir.clone();
    let bg_call_id = call_id.clone();
    let bg_tool_name = tool_name.clone();
    let bg_tool_name_for_event = tool_name.clone();
    let register_task_id = bg_task_id.clone();
    let register_session_id = runtime.turn.shared.session_id.clone();

    let handle = tokio::spawn(async move {
        let bg_task_id = bg_task_id_for_closure;
        // Phase 1: 执行工具
        let result = tool_registry.execute(&name, tool_input, &tool_ctx).await;
        *slot_writer.lock() = Some(result);

        // 通知主线程执行完成（用于快速路径检测）
        let _ = done_tx.send(());

        // 仅在实际转入后台后继续 Phase 2；快速路径会 drop sender，此处收到 Err 后直接退出。
        if backgrounded_rx.await.is_err() {
            return;
        }

        // Phase 2: 后台路径 — 持久化 + 发事件 + 移除
        let raw = slot_writer.lock().take();
        let mut result = match raw {
            Some(Ok(mut r)) => {
                r.call_id = bg_call_id.clone();
                r.duration_ms = Some(started_at.elapsed().as_millis() as u64);
                r
            },
            Some(Err(e)) => {
                error_tool_result(bg_call_id.clone(), &bg_tool_name, e, started_at.elapsed())
            },
            None => error_tool_result(
                bg_call_id.clone(),
                &bg_tool_name,
                ToolError::Execution("background task completed but no result available".into()),
                started_at.elapsed(),
            ),
        };

        // 在结果元数据中标记后台来源
        result
            .metadata
            .insert("task_id".into(), serde_json::json!(bg_task_id.to_string()));

        // 持久化输出到磁盘
        let original_content = result.content.clone();
        let output_bytes = if let Some(ref dir) = bg_store_dir {
            let bg_dir = dir.join("background-tasks");
            match astrcode_storage::tool_artifacts::write_background_task_file(
                &bg_dir,
                bg_task_id.as_str(),
                &original_content,
                astrcode_storage::tool_artifacts::DEFAULT_BG_TASK_OUTPUT_LIMIT,
            ) {
                Ok(bytes) => {
                    tracing::debug!(
                        task_id = %bg_task_id,
                        bytes,
                        "background task output persisted"
                    );
                    Some(bytes)
                },
                Err(e) => {
                    tracing::warn!(
                        task_id = %bg_task_id,
                        error = %e,
                        "failed to persist background task output; keeping inline"
                    );
                    None
                },
            }
        } else {
            None
        };

        // 将 result.content 替换为轻量摘要（持久化成功时）
        if let Some(bytes) = output_bytes {
            let status = if result.is_error {
                "failed"
            } else {
                "completed"
            };
            let duration = result
                .duration_ms
                .map(|ms| format!(" ({ms}ms)"))
                .unwrap_or_default();
            result.content = format!(
                "Background task {status} (task: {bg_task_id}, output: {bytes} \
                 bytes){duration}.\nUse `task action=result taskId=\"{bg_task_id}\"` to read the \
                 output."
            );
            result
                .metadata
                .insert("outputPersisted".into(), serde_json::json!(true));
        }

        tracing::info!(
            tool_name = bg_tool_name,
            call_id = bg_call_id,
            task_id = %bg_task_id,
            is_error = result.is_error,
            output_persisted = output_bytes.is_some(),
            "background task completed"
        );

        // 发出 BackgroundTaskNotification（durable）+ BackgroundTaskCompleted（live）
        let notification = EventPayload::BackgroundTaskNotification {
            task_id: bg_task_id.clone(),
            call_id: ToolCallId::from(result.call_id.clone()),
            tool_name: bg_tool_name.clone(),
            summary: result.content.clone(),
        };

        if let Err(e) = bg_session.emit_durable(None, notification.clone()).await {
            tracing::warn!(
                session_id = %bg_session.id(),
                error = %e,
                "background task: persist notification failed; sending live fallback"
            );
            bg_session.emit_live(None, notification).await;
        }

        bg_session
            .emit_live(
                None,
                EventPayload::BackgroundTaskCompleted {
                    task_id: bg_task_id.clone(),
                    call_id: ToolCallId::from(result.call_id.clone()),
                    tool_name: bg_tool_name,
                    result,
                },
            )
            .await;

        // 从管理器移除自身
        bg_manager.lock().remove(&bg_task_id);
    });

    // 注册到后台任务管理器
    {
        let mut mgr = runtime.turn.capabilities.background_tasks.lock();
        mgr.register(register_task_id, register_session_id, handle);
    }

    // 用 timeout 等待完成通知或超时
    let wait_result = tokio::select! {
        _ = runtime.cancellation_token.cancelled() => {
            // 取消：通过 task_id 从 manager abort
            runtime.turn.capabilities.background_tasks.lock().cancel(&bg_task_id_for_cancel);
            return (
                call_index,
                interrupted_tool_result(call_id.clone(), &tool_name, started_at.elapsed()),
            );
        },
        result = tokio::time::timeout(std::time::Duration::from_secs(threshold_secs), done_rx) => result,
    };

    match wait_result {
        Ok(Ok(())) => {
            // 在阈值内完成 — 快速路径：通知 task 不要跑 Phase 2，取走结果并注销。
            drop(backgrounded_tx);
            runtime
                .turn
                .capabilities
                .background_tasks
                .lock()
                .remove(&bg_task_id);
            match result_slot.lock().take() {
                Some(Ok(mut r)) => {
                    r.call_id = call_id.clone();
                    r.duration_ms = Some(started_at.elapsed().as_millis() as u64);
                    tracing::debug!(
                        tool_name,
                        call_id,
                        duration_ms = r.duration_ms.unwrap_or_default(),
                        "tool execution completed (before background threshold)"
                    );
                    (call_index, r)
                },
                Some(Err(e)) => (
                    call_index,
                    error_tool_result(call_id.clone(), &tool_name, e, started_at.elapsed()),
                ),
                None => {
                    tracing::error!(
                        tool_name,
                        call_id,
                        "done_tx sent but no result available in slot"
                    );
                    (
                        call_index,
                        error_tool_result(
                            call_id.clone(),
                            &tool_name,
                            ToolError::Execution(
                                "tool task completed but no result available".into(),
                            ),
                            started_at.elapsed(),
                        ),
                    )
                },
            }
        },
        Ok(Err(_)) => {
            // done_tx dropped — task panicked
            tracing::error!(tool_name, call_id, "tool execution task panicked");
            (
                call_index,
                error_tool_result(
                    call_id.clone(),
                    &tool_name,
                    ToolError::Execution("tool task panicked before completion".into()),
                    started_at.elapsed(),
                ),
            )
        },
        Err(_) => {
            // 超时 — 后台路径。通知 task 在 Phase 1 完成后进入 Phase 2。
            let _ = backgrounded_tx.send(());
            tracing::info!(
                tool_name,
                call_id,
                task_id = %bg_task_id,
                threshold_secs,
                "tool execution moved to background"
            );

            let bg_reason: String = match threshold_secs {
                0 => "explicit".into(),
                _ => "auto_threshold".into(),
            };
            runtime
                .publisher
                .live(EventPayload::ToolCallBackgrounded {
                    call_id: ToolCallId::from(call_id.as_str()),
                    tool_name: bg_tool_name_for_event,
                    task_id: bg_task_id.clone(),
                    reason: bg_reason,
                })
                .await;

            let command = call
                .tool_input
                .get("command")
                .and_then(|v| v.as_str())
                .map(String::from);
            let placeholder =
                backgrounded_placeholder_result(&call_id, &bg_task_id, command.as_deref());
            (call_index, placeholder)
        },
    }
}

// ─── File observation store ──────────────────────────────────────────────────

/// 进程内文件观察存储，用于 read/edit 工具的 read-before-edit 守卫。
///
/// 以规范化路径为 key 记录最近一次 `read` 或成功 `edit` 后的文件快照。
/// 生命周期与 session 一致（由 `TurnRunner` 创建，随 `TurnRunner` 销毁）。
#[derive(Default)]
pub struct InMemoryFileObservationStore {
    observations: Mutex<std::collections::HashMap<String, FileObservation>>,
}

impl FileObservationStore for InMemoryFileObservationStore {
    fn remember(&self, observation: FileObservation) {
        let mut map = self.observations.lock();
        map.insert(observation.path.clone(), observation);
    }

    fn load(&self, path: &str) -> Option<FileObservation> {
        let map = self.observations.lock();
        map.get(path).cloned()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use astrcode_core::{
        config::{ContextSettings, EffectiveConfig, ExtensionSettings, LlmSettings, OpenAiApiMode},
        event::EventPayload,
        llm::{LlmError, LlmEvent, LlmMessage, LlmProvider, ModelLimits},
        storage::EventStore,
        tool::{
            BackgroundPolicy, ExecutionMode, Tool, ToolDefinition, ToolError, ToolOrigin,
            ToolResult,
        },
        types::{ToolCallId, new_session_id, new_turn_id},
    };
    use astrcode_extensions::runner::ExtensionRunner;
    use astrcode_storage::in_memory::InMemoryEventStore;
    use astrcode_tools::registry::ToolRegistry;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::{
        session::{Session, SessionCreateParams},
        session_runtime::SessionRuntimeState,
        session_runtime_services::SessionRuntimeServices,
        tool_types::ExecutableToolCall,
    };

    struct UnusedLlm;

    #[async_trait::async_trait]
    impl LlmProvider for UnusedLlm {
        async fn generate(
            &self,
            _messages: Vec<LlmMessage>,
            _tools: Vec<ToolDefinition>,
        ) -> Result<mpsc::UnboundedReceiver<LlmEvent>, LlmError> {
            unreachable!()
        }

        fn model_limits(&self) -> ModelLimits {
            ModelLimits {
                max_input_tokens: 1024,
                max_output_tokens: 1024,
            }
        }
    }

    fn test_caps() -> Arc<SessionRuntimeServices> {
        let llm: Arc<dyn LlmProvider> = Arc::new(UnusedLlm);
        let extension_runner = Arc::new(ExtensionRunner::new(std::time::Duration::from_secs(1)));
        let context_assembler = Arc::new(
            astrcode_context::context_assembler::LlmContextAssembler::new(
                ContextSettings::default(),
            ),
        );
        let effective = EffectiveConfig {
            llm: LlmSettings {
                provider_kind: "mock".into(),
                base_url: String::new(),
                api_key: String::new(),
                api_mode: OpenAiApiMode::ChatCompletions,
                model_id: "mock-model".into(),
                max_tokens: 1024,
                context_limit: 1024,
                connect_timeout_secs: 1,
                read_timeout_secs: 1,
                max_retries: 0,
                retry_base_delay_ms: 0,
                supports_prompt_cache_key: false,
                prompt_cache_retention: None,
                reasoning: false,
                thinking_level: None,
            },
            small_llm: LlmSettings {
                provider_kind: "mock".into(),
                base_url: String::new(),
                api_key: String::new(),
                api_mode: OpenAiApiMode::ChatCompletions,
                model_id: "mock-model".into(),
                max_tokens: 1024,
                context_limit: 1024,
                connect_timeout_secs: 1,
                read_timeout_secs: 1,
                max_retries: 0,
                retry_base_delay_ms: 0,
                supports_prompt_cache_key: false,
                prompt_cache_retention: None,
                reasoning: false,
                thinking_level: None,
            },
            context: ContextSettings::default(),
            agent: Default::default(),
            extensions: ExtensionSettings::default(),
        };
        Arc::new(SessionRuntimeServices::new(
            llm.clone(),
            llm,
            extension_runner,
            context_assembler,
            effective,
        ))
    }

    async fn test_session(store: Arc<dyn EventStore>) -> Session {
        let caps = test_caps();
        let sid = new_session_id();
        let runtime = Arc::new(SessionRuntimeState::new(
            caps.llm(),
            caps.small_llm(),
            "mock-model".into(),
        ));
        Session::create_with_params(SessionCreateParams {
            store,
            sid,
            working_dir: std::env::temp_dir().to_string_lossy().into_owned(),
            model_id: "mock-model".into(),
            parent: None,
            tool_policy: None,
            source_extension: None,
            runtime,
            caps,
        })
        .await
        .unwrap()
    }

    async fn runtime_context(session: &Session) -> ToolCallRuntimeContext {
        let model = session.read_model().await.unwrap();
        let store_dir = session.session_store_dir().await;
        let turn = TurnToolContext::for_turn(session, &model, store_dir);
        let turn_id = new_turn_id();
        ToolCallRuntimeContext {
            turn,
            tools: vec![],
            tool_result_reader: Some(Arc::new(session.clone()) as Arc<dyn ToolResultArtifactReader>),
            publisher: Arc::new(TurnEvents::new(session.clone(), turn_id, None)),
            cancellation_token: CancellationToken::new(),
            session: session.clone(),
        }
    }

    struct ImmediateBackgroundTool;

    #[async_trait::async_trait]
    impl Tool for ImmediateBackgroundTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "immediate_bg".into(),
                description: String::new(),
                parameters: serde_json::json!({"type": "object"}),
                origin: ToolOrigin::Builtin,
                execution_mode: ExecutionMode::Sequential,
            }
        }

        fn background_policy(&self) -> BackgroundPolicy {
            BackgroundPolicy::AutoAfter { threshold_secs: 60 }
        }

        async fn execute(
            &self,
            _arguments: serde_json::Value,
            _ctx: &ToolExecutionContext,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult {
                call_id: String::new(),
                content: "fast-ok".into(),
                is_error: false,
                error: None,
                metadata: Default::default(),
                duration_ms: None,
            })
        }
    }

    struct SlowBackgroundTool {
        delay: std::time::Duration,
    }

    #[async_trait::async_trait]
    impl Tool for SlowBackgroundTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "slow_bg".into(),
                description: String::new(),
                parameters: serde_json::json!({"type": "object"}),
                origin: ToolOrigin::Builtin,
                execution_mode: ExecutionMode::Sequential,
            }
        }

        fn background_policy(&self) -> BackgroundPolicy {
            BackgroundPolicy::AutoAfter { threshold_secs: 0 }
        }

        async fn execute(
            &self,
            _arguments: serde_json::Value,
            _ctx: &ToolExecutionContext,
        ) -> Result<ToolResult, ToolError> {
            tokio::time::sleep(self.delay).await;
            Ok(ToolResult {
                call_id: String::new(),
                content: "slow-done".into(),
                is_error: false,
                error: None,
                metadata: Default::default(),
                duration_ms: None,
            })
        }
    }

    #[tokio::test]
    async fn background_tool_fast_path_returns_inline_result_without_notification() {
        let store: Arc<dyn EventStore> = Arc::new(InMemoryEventStore::new());
        let session = test_session(Arc::clone(&store)).await;
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(ImmediateBackgroundTool));
        let registry = Arc::new(registry);

        let runtime = runtime_context(&session).await;
        let call = ExecutableToolCall {
            index: 0,
            call_id: "call-fast".into(),
            name: "immediate_bg".into(),
            tool_input: serde_json::json!({}),
        };

        let (_idx, result) = execute_tool_call(registry, runtime, call).await;
        assert_eq!(result.content, "fast-ok");
        assert!(!result.is_error);

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let events = store.replay_events(session.id()).await.unwrap();
        assert!(!events.iter().any(|event| matches!(
            event.payload,
            EventPayload::BackgroundTaskNotification { .. }
        )));
    }

    #[tokio::test]
    async fn background_tool_timeout_path_emits_notification_on_completion() {
        let store: Arc<dyn EventStore> = Arc::new(InMemoryEventStore::new());
        let session = test_session(Arc::clone(&store)).await;
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(SlowBackgroundTool {
            delay: std::time::Duration::from_millis(200),
        }));
        let registry = Arc::new(registry);

        let runtime = runtime_context(&session).await;
        let call = ExecutableToolCall {
            index: 0,
            call_id: "call-slow".into(),
            name: "slow_bg".into(),
            tool_input: serde_json::json!({}),
        };

        let (_idx, result) = execute_tool_call(registry, runtime, call).await;
        assert!(result.content.contains("Task moved to background"));
        assert_eq!(
            result.metadata.get("backgrounded"),
            Some(&serde_json::json!(true))
        );

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let events = store.replay_events(session.id()).await.unwrap();
        assert!(events.iter().any(|event| matches!(
            &event.payload,
            EventPayload::BackgroundTaskNotification {
                call_id,
                ..
            } if call_id == &ToolCallId::from("call-slow")
        )));
    }

    #[test]
    fn resolve_effective_policy_explicit_true() {
        let input = serde_json::json!({ "runInBackground": true });
        let result =
            resolve_effective_policy(BackgroundPolicy::AutoAfter { threshold_secs: 60 }, &input);
        assert_eq!(result, BackgroundPolicy::AutoAfter { threshold_secs: 0 });
    }

    #[test]
    fn resolve_effective_policy_explicit_false() {
        let input = serde_json::json!({ "runInBackground": false });
        let result =
            resolve_effective_policy(BackgroundPolicy::AutoAfter { threshold_secs: 60 }, &input);
        assert_eq!(result, BackgroundPolicy::Never);
    }

    #[test]
    fn resolve_effective_policy_missing_field_returns_declared() {
        let input = serde_json::json!({ "command": "echo hi" });
        let result =
            resolve_effective_policy(BackgroundPolicy::AutoAfter { threshold_secs: 60 }, &input);
        assert_eq!(result, BackgroundPolicy::AutoAfter { threshold_secs: 60 });
    }

    #[test]
    fn resolve_effective_policy_declared_never_with_override() {
        let input = serde_json::json!({ "runInBackground": true });
        let result = resolve_effective_policy(BackgroundPolicy::Never, &input);
        assert_eq!(result, BackgroundPolicy::AutoAfter { threshold_secs: 0 });
    }

    #[test]
    fn resolve_effective_policy_non_bool_is_none() {
        let input = serde_json::json!({ "runInBackground": "yes" });
        let result =
            resolve_effective_policy(BackgroundPolicy::AutoAfter { threshold_secs: 30 }, &input);
        assert_eq!(result, BackgroundPolicy::AutoAfter { threshold_secs: 30 });
    }

    #[test]
    fn error_tool_result_not_found() {
        let result = error_tool_result(
            "call-1".into(),
            "my_tool",
            ToolError::NotFound("missing".into()),
            std::time::Duration::from_millis(50),
        );
        assert_eq!(result.call_id, "call-1");
        assert!(result.is_error);
        assert!(result.content.contains("missing"));
        assert!(result.content.contains("Suggestion"));
    }

    #[test]
    fn error_tool_result_not_found_suggests_glob_for_legacy_find() {
        let result = error_tool_result(
            "call-2".into(),
            "find",
            ToolError::NotFound("find".into()),
            std::time::Duration::from_millis(10),
        );
        assert!(result.content.contains("glob"));
    }

    #[test]
    fn error_tool_result_timeout_includes_ms() {
        let result = error_tool_result(
            "call-2".into(),
            "shell",
            ToolError::Timeout(5000),
            std::time::Duration::from_millis(5000),
        );
        assert!(result.content.contains("5000ms"));
        assert_eq!(result.metadata["timeoutMs"], serde_json::json!(5000));
    }

    #[test]
    fn error_tool_result_blocked() {
        let result = error_tool_result(
            "call-3".into(),
            "shell",
            ToolError::Blocked {
                reason: "policy reason".into(),
            },
            std::time::Duration::from_millis(10),
        );
        assert!(result.content.contains("blocked"));
        assert!(result.content.contains("policy reason"));
    }
}

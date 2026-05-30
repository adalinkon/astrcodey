//! 后台任务管理器与占位结果构造。
//!
//! 管理被自动后台化的工具调用（主要是长时间运行的 shell 命令）。
//! 提供注册、取消、查询和清理能力。

use std::{collections::HashMap, sync::Arc};

use astrcode_core::{
    storage::{BackgroundTaskOutputSlice, StorageError},
    tool::{BackgroundTaskReader, ToolResult},
    types::{BackgroundTaskId, SessionId},
};
use parking_lot::Mutex;

struct RunningTask {
    session_id: SessionId,
    handle: tokio::task::JoinHandle<()>,
}

/// 管理所有 session 的后台任务。
///
/// 当工具执行超过阈值时，agent loop 将其转入后台，把单个 JoinHandle 注册到这里。
/// cancel 会 abort 整个任务生命周期（执行 + 持久化 + 通知）。完成后任务自行移除。
pub struct BackgroundTasks {
    tasks: HashMap<BackgroundTaskId, RunningTask>,
}

impl BackgroundTasks {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }

    /// 注册一个后台任务。
    ///
    /// `task_id` 由调用方提前生成，保证与 ToolCallBackgrounded 事件和占位结果中的 ID 一致。
    pub fn register(
        &mut self,
        task_id: BackgroundTaskId,
        session_id: SessionId,
        handle: tokio::task::JoinHandle<()>,
    ) {
        self.tasks
            .insert(task_id, RunningTask { session_id, handle });
    }

    /// 移除已完成的任务（由任务自身在完成后调用）。
    pub fn remove(&mut self, task_id: &BackgroundTaskId) {
        self.tasks.remove(task_id);
    }

    /// 取消并移除一个后台任务。
    pub fn cancel(&mut self, task_id: &BackgroundTaskId) -> bool {
        if let Some(task) = self.tasks.remove(task_id) {
            task.handle.abort();
            true
        } else {
            false
        }
    }

    /// 清理指定 session 的所有后台任务（session 结束或删除时调用）。
    pub fn cleanup_session(&mut self, session_id: &SessionId) {
        let to_remove: Vec<BackgroundTaskId> = self
            .tasks
            .iter()
            .filter(|(_, task)| &task.session_id == session_id)
            .map(|(id, _)| id.clone())
            .collect();
        for task_id in to_remove {
            if let Some(task) = self.tasks.remove(&task_id) {
                task.handle.abort();
            }
        }
    }

    /// 列出指定会话的所有活跃后台任务 ID。
    pub fn list_active(&self, session_id: &SessionId) -> Vec<BackgroundTaskId> {
        self.tasks
            .iter()
            .filter(|(_, task)| &task.session_id == session_id)
            .map(|(id, _)| id.clone())
            .collect()
    }
}

/// 将 `BackgroundTasks` 适配为 `BackgroundTaskReader` trait。
///
/// 这个薄包装器让 `TaskTool` 能通过 `ToolExecutionContext` 读取后台任务状态，
/// 而不暴露 `BackgroundTasks` 的内部方法（如 `register`、`cleanup_session`）。
pub struct BackgroundTaskReaderImpl {
    manager: Arc<Mutex<BackgroundTasks>>,
    session_store_dir: Option<std::path::PathBuf>,
}

impl BackgroundTaskReaderImpl {
    pub fn new(
        manager: Arc<Mutex<BackgroundTasks>>,
        session_store_dir: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            manager,
            session_store_dir,
        }
    }
}

impl BackgroundTaskReader for BackgroundTaskReaderImpl {
    fn list_active(&self, session_id: &SessionId) -> Vec<BackgroundTaskId> {
        self.manager.lock().list_active(session_id)
    }

    fn cancel(&self, session_id: &SessionId, task_id: &BackgroundTaskId) -> bool {
        let mut mgr = self.manager.lock();
        if mgr
            .tasks
            .get(task_id)
            .is_some_and(|t| &t.session_id == session_id)
        {
            mgr.cancel(task_id)
        } else {
            false
        }
    }

    fn read_output(
        &self,
        _session_id: &SessionId,
        task_id: &BackgroundTaskId,
        char_offset: usize,
        max_chars: usize,
    ) -> Result<BackgroundTaskOutputSlice, StorageError> {
        let dir = self
            .session_store_dir
            .as_ref()
            .ok_or_else(|| {
                StorageError::Unsupported("session store directory not available".into())
            })?
            .join("background-tasks");
        astrcode_storage::tool_artifacts::read_background_task_file(
            &dir,
            task_id.as_str(),
            char_offset,
            max_chars,
        )
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                StorageError::NotFound(_session_id.clone())
            } else {
                StorageError::Io(e)
            }
        })
    }
}

impl Default for BackgroundTasks {
    fn default() -> Self {
        Self::new()
    }
}

/// 为后台化的工具调用构造占位 `ToolResult`。
///
/// LLM 收到这个结果后会知道任务已在后台运行，可以继续其他推理。
pub fn backgrounded_placeholder_result(
    call_id: &str,
    task_id: &BackgroundTaskId,
    command: Option<&str>,
) -> ToolResult {
    let mut content = format!(
        "Task moved to background (task: {task_id}). Output will be persisted on completion.\nUse \
         `task action=result taskId=\"{task_id}\"` to read the output once done."
    );
    if let Some(cmd) = command {
        content = format!("{content} Command: {cmd}");
    }

    let mut meta = std::collections::BTreeMap::new();
    meta.insert("backgrounded".into(), serde_json::json!(true));
    meta.insert("task_id".into(), serde_json::json!(task_id.to_string()));

    ToolResult {
        call_id: call_id.to_string(),
        content,
        is_error: false,
        error: None,
        metadata: meta,
        duration_ms: None,
    }
}

#[cfg(test)]
mod tests {
    use astrcode_core::{
        config::{EffectiveConfig, ExtensionSettings, LlmSettings, OpenAiApiMode},
        event::Event,
        llm::{LlmError, LlmEvent, LlmMessage, LlmProvider, ModelLimits},
        storage::{EventReader, EventStore, SessionReadModel, SessionSummary, StorageError},
        tool::ToolDefinition,
        types::{Cursor, ToolCallId},
    };
    use astrcode_extensions::runner::ExtensionRunner;
    use astrcode_storage::in_memory::InMemoryEventStore;
    use tokio::sync::mpsc;

    use super::*;
    use crate::session_runtime::SessionRuntimeState;

    struct NeverLlm;

    #[async_trait::async_trait]
    impl LlmProvider for NeverLlm {
        async fn generate(
            &self,
            _messages: Vec<LlmMessage>,
            _tools: Vec<ToolDefinition>,
        ) -> Result<mpsc::UnboundedReceiver<LlmEvent>, LlmError> {
            std::future::pending().await
        }

        fn model_limits(&self) -> ModelLimits {
            ModelLimits {
                max_input_tokens: 1024,
                max_output_tokens: 1024,
            }
        }
    }

    struct FailToolCompletionStore {
        inner: InMemoryEventStore,
    }

    impl FailToolCompletionStore {
        fn new() -> Self {
            Self {
                inner: InMemoryEventStore::new(),
            }
        }
    }

    #[async_trait::async_trait]
    impl EventReader for FailToolCompletionStore {
        async fn replay_events(&self, session_id: &SessionId) -> Result<Vec<Event>, StorageError> {
            self.inner.replay_events(session_id).await
        }

        async fn session_read_model(
            &self,
            session_id: &SessionId,
        ) -> Result<SessionReadModel, StorageError> {
            self.inner.session_read_model(session_id).await
        }

        async fn session_system_prompt(
            &self,
            session_id: &SessionId,
        ) -> Result<Option<String>, StorageError> {
            self.inner.session_system_prompt(session_id).await
        }

        async fn list_session_summaries(&self) -> Result<Vec<SessionSummary>, StorageError> {
            self.inner.list_session_summaries().await
        }

        async fn latest_cursor(
            &self,
            session_id: &SessionId,
        ) -> Result<Option<Cursor>, StorageError> {
            self.inner.latest_cursor(session_id).await
        }

        async fn replay_from(
            &self,
            session_id: &SessionId,
            cursor: &Cursor,
        ) -> Result<Vec<Event>, StorageError> {
            self.inner.replay_from(session_id, cursor).await
        }

        async fn list_sessions(&self) -> Result<Vec<SessionId>, StorageError> {
            self.inner.list_sessions().await
        }

        async fn read_tool_result_artifact_by_path(
            &self,
            session_id: &SessionId,
            path: &str,
            char_offset: usize,
            max_chars: usize,
        ) -> Result<astrcode_core::storage::ToolResultArtifactSlice, StorageError> {
            self.inner
                .read_tool_result_artifact_by_path(session_id, path, char_offset, max_chars)
                .await
        }

        async fn session_store_dir(
            &self,
            session_id: &SessionId,
        ) -> Result<Option<std::path::PathBuf>, StorageError> {
            self.inner.session_store_dir(session_id).await
        }
    }

    #[async_trait::async_trait]
    impl EventStore for FailToolCompletionStore {
        async fn create_session(
            &self,
            session_id: &SessionId,
            working_dir: &str,
            model_id: &str,
            parent_session_id: Option<&SessionId>,
            tool_policy: Option<&astrcode_core::extension::ChildToolPolicy>,
            source_extension: Option<&str>,
        ) -> Result<Event, StorageError> {
            self.inner
                .create_session(
                    session_id,
                    working_dir,
                    model_id,
                    parent_session_id,
                    tool_policy,
                    source_extension,
                )
                .await
        }

        async fn append_event(&self, event: Event) -> Result<Event, StorageError> {
            if matches!(
                event.payload,
                astrcode_core::event::EventPayload::BackgroundTaskNotification { .. }
            ) {
                return Err(StorageError::Unsupported("forced append failure".into()));
            }
            self.inner.append_event(event).await
        }

        async fn checkpoint(
            &self,
            session_id: &SessionId,
            cursor: &Cursor,
        ) -> Result<(), StorageError> {
            self.inner.checkpoint(session_id, cursor).await
        }

        async fn delete_session(&self, session_id: &SessionId) -> Result<(), StorageError> {
            self.inner.delete_session(session_id).await
        }
    }

    fn test_caps() -> Arc<crate::session_runtime_services::SessionRuntimeServices> {
        let llm: Arc<dyn LlmProvider> = Arc::new(NeverLlm);
        let extension_runner = Arc::new(ExtensionRunner::new(std::time::Duration::from_secs(1)));
        let context_assembler = Arc::new(
            astrcode_context::context_assembler::LlmContextAssembler::new(Default::default()),
        );
        Arc::new(
            crate::session_runtime_services::SessionRuntimeServices::new(
                Arc::clone(&llm),
                llm,
                extension_runner,
                context_assembler,
                EffectiveConfig {
                    llm: LlmSettings {
                        provider_kind: "mock".into(),
                        base_url: String::new(),
                        api_key: String::new(),
                        api_mode: OpenAiApiMode::ChatCompletions,
                        model_id: "mock".into(),
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
                        model_id: "mock".into(),
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
                    context: Default::default(),
                    agent: Default::default(),
                    extensions: ExtensionSettings::default(),
                },
            ),
        )
    }

    fn fake_handle() -> tokio::task::JoinHandle<()> {
        tokio::spawn(async {})
    }

    #[tokio::test]
    async fn cancel_removes_task_and_returns_true() {
        let mut mgr = BackgroundTasks::new();
        let task_id = BackgroundTaskId::from("task-1");
        let session_id = SessionId::from("session-1");
        let handle = fake_handle();
        mgr.register(task_id.clone(), session_id.clone(), handle);

        assert!(mgr.cancel(&task_id));
        assert!(mgr.list_active(&session_id).is_empty());
    }

    #[test]
    fn cancel_returns_false_for_unknown_task() {
        let mut mgr = BackgroundTasks::new();
        let task_id = BackgroundTaskId::from("nonexistent");
        assert!(!mgr.cancel(&task_id));
    }

    #[tokio::test]
    async fn cleanup_session_removes_all_tasks_for_session() {
        let mut mgr = BackgroundTasks::new();
        let session_a = SessionId::from("session-a");
        let session_b = SessionId::from("session-b");

        for i in 0..3 {
            let handle = fake_handle();
            mgr.register(
                BackgroundTaskId::from(format!("task-a-{i}")),
                session_a.clone(),
                handle,
            );
        }
        let handle_b = fake_handle();
        mgr.register(
            BackgroundTaskId::from("task-b-0"),
            session_b.clone(),
            handle_b,
        );

        mgr.cleanup_session(&session_a);
        assert!(mgr.list_active(&session_a).is_empty());
        assert_eq!(mgr.list_active(&session_b).len(), 1);
    }

    #[tokio::test]
    async fn list_active_returns_only_matching_session() {
        let mut mgr = BackgroundTasks::new();
        let session_a = SessionId::from("session-a");
        let session_b = SessionId::from("session-b");

        let h1 = fake_handle();
        let h2 = fake_handle();
        mgr.register(BackgroundTaskId::from("task-1"), session_a.clone(), h1);
        mgr.register(BackgroundTaskId::from("task-2"), session_b.clone(), h2);

        let active_a = mgr.list_active(&session_a);
        assert_eq!(active_a.len(), 1);
        assert_eq!(active_a[0], BackgroundTaskId::from("task-1"));
    }

    #[tokio::test]
    async fn background_forwarder_emits_durable_and_live_on_completion() {
        let store: Arc<dyn EventStore> = Arc::new(InMemoryEventStore::new());
        let session_id = SessionId::from("test-bg-forwarder");
        store
            .create_session(&session_id, "/tmp", "mock", None, None, None)
            .await
            .unwrap();

        let runtime = Arc::new(SessionRuntimeState::new(
            Arc::new(NeverLlm),
            Arc::new(NeverLlm),
            "mock".into(),
        ));
        let caps = test_caps();
        let session = crate::session::Session {
            id: session_id.clone(),
            store: Arc::clone(&store),
            runtime: Arc::clone(&runtime),
            caps,
        };

        let bg_manager = runtime.background_tasks();
        let task_id = BackgroundTaskId::from("bg-task-1");
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

        let bg_task_id = task_id.clone();
        let bg_session = session.clone();
        let bg_mgr = Arc::clone(&bg_manager);
        let handle = tokio::spawn(async move {
            // 模拟工具执行完成
            drop(done_rx);

            // 直接 emit durable + live（模拟新的单 task 模型）
            let notification = astrcode_core::event::EventPayload::BackgroundTaskNotification {
                task_id: bg_task_id.clone(),
                call_id: ToolCallId::from("call-1"),
                tool_name: "shell".into(),
                summary: "done".into(),
            };
            let _ = bg_session.emit_durable(None, notification).await;
            bg_session
                .emit_live(
                    None,
                    astrcode_core::event::EventPayload::BackgroundTaskCompleted {
                        task_id: bg_task_id.clone(),
                        call_id: ToolCallId::from("call-1"),
                        tool_name: "shell".into(),
                        result: astrcode_core::tool::ToolResult {
                            call_id: "call-1".into(),
                            content: "done".into(),
                            is_error: false,
                            error: None,
                            metadata: Default::default(),
                            duration_ms: Some(100),
                        },
                    },
                )
                .await;

            bg_mgr.lock().remove(&bg_task_id);
        });

        bg_manager
            .lock()
            .register(task_id.clone(), session_id.clone(), handle);

        // 通知执行完成
        let _ = done_tx.send(());

        // 等待任务完成
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // 验证 durable event 已持久化
        let events = store.replay_events(&session_id).await.unwrap();
        let has_notification = events.iter().any(|e| {
            matches!(
                &e.payload,
                astrcode_core::event::EventPayload::BackgroundTaskNotification { task_id: tid, .. }
                if tid == &task_id
            )
        });
        assert!(
            has_notification,
            "should have persisted BackgroundTaskNotification"
        );

        // 验证任务已从 manager 移除
        assert!(bg_manager.lock().list_active(&session_id).is_empty());
    }

    #[tokio::test]
    async fn background_forwarder_falls_back_to_live_on_durable_failure() {
        let store: Arc<dyn EventStore> = Arc::new(FailToolCompletionStore::new());
        let session_id = SessionId::from("test-bg-fallback");
        store
            .create_session(&session_id, "/tmp", "mock", None, None, None)
            .await
            .unwrap();

        let runtime = Arc::new(SessionRuntimeState::new(
            Arc::new(NeverLlm),
            Arc::new(NeverLlm),
            "mock".into(),
        ));
        let caps = test_caps();
        let session = crate::session::Session {
            id: session_id.clone(),
            store: Arc::clone(&store),
            runtime: Arc::clone(&runtime),
            caps,
        };

        let bg_manager = runtime.background_tasks();
        let task_id = BackgroundTaskId::from("bg-task-fail");
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

        let bg_task_id = task_id.clone();
        let bg_session = session.clone();
        let bg_mgr = Arc::clone(&bg_manager);
        let handle = tokio::spawn(async move {
            drop(done_rx);

            let notification = astrcode_core::event::EventPayload::BackgroundTaskNotification {
                task_id: bg_task_id.clone(),
                call_id: ToolCallId::from("call-1"),
                tool_name: "shell".into(),
                summary: "done".into(),
            };
            // durable 写入会失败（FailToolCompletionStore 拒绝 BackgroundTaskNotification）
            if bg_session
                .emit_durable(None, notification.clone())
                .await
                .is_err()
            {
                bg_session.emit_live(None, notification).await;
            }

            bg_mgr.lock().remove(&bg_task_id);
        });

        bg_manager
            .lock()
            .register(task_id.clone(), session_id.clone(), handle);

        let _ = done_tx.send(());
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // durable 失败但 task 应该已从 manager 移除（live fallback 成功）
        assert!(bg_manager.lock().list_active(&session_id).is_empty());
    }

    #[tokio::test]
    async fn reader_cancel_rejects_wrong_session() {
        let manager = Arc::new(Mutex::new(BackgroundTasks::new()));
        let reader = BackgroundTaskReaderImpl::new(Arc::clone(&manager), None);

        let task_id = BackgroundTaskId::from("task-x");
        let session_correct = SessionId::from("correct");
        let session_wrong = SessionId::from("wrong");

        let handle = fake_handle();
        manager
            .lock()
            .register(task_id.clone(), session_correct.clone(), handle);

        assert!(!reader.cancel(&session_wrong, &task_id));
        assert!(reader.cancel(&session_correct, &task_id));
    }
}

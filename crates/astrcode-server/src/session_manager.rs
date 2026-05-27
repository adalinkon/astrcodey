use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use astrcode_core::{
    event::{Event, EventPayload},
    extension::ExtensionEvent,
    lifecycle::SessionResourceCleanup,
    storage::{EventStore, StorageError},
    types::{Cursor, SessionId},
};
use astrcode_session::{
    Session, SessionCreateParams, SessionError, SessionRuntimeServices, SessionRuntimeState,
    session::emit_lifecycle_for_read_model,
};
use astrcode_tools::registry::ToolRegistry;
use parking_lot::Mutex;

use crate::{
    config_manager::ConfigManager, server_event_bus::ServerEventBus, turn_scheduler::TurnScheduler,
};

pub(crate) struct CreatedSession {
    pub(crate) session: Session,
    pub(crate) start_event: Event,
}

pub(crate) struct ForkedSession {
    pub(crate) session: Session,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionManagerError {
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Extension(#[from] astrcode_core::extension::ExtensionError),
    #[error("session created but no events found")]
    MissingStartEvent,
    #[error("invalid fork cursor: {0}")]
    InvalidCursor(String),
}

/// Server 侧的 session 生命周期门面。
///
/// **职责边界（避免与 [`Session`] / [`EventStore`] 重复）：**
/// - `EventStore`：durable 读写（投影、列表、replay）→ 直接用 [`Self::event_store`]
/// - `Session`：单 session 操作（submit、spawn_child、emit）→ 经 [`Self::open`]
/// - `SessionManager`：进程内 sid→runtime 映射、open 并发锁、delete/recycle 资源清理
///
/// durable session 仍由 [`Session`] / [`EventStore`] 负责；这里集中管理
/// 与 session 同生灭的进程内资源，避免 handler 逐项记忆清理细节。
///
/// 后台任务（`BackgroundTaskManager`）现在由 `SessionRuntimeState` 持有，
/// 跟着 session 走；SessionManager 不再持有全局副本。删除 session 时通过
/// `runtime_states` 找到对应 runtime 并清理它的 bg_tasks。
pub struct SessionManager {
    event_store: Arc<dyn EventStore>,
    config: Arc<ConfigManager>,
    runtime_states: Mutex<HashMap<SessionId, Arc<SessionRuntimeState>>>,
    open_locks: Mutex<HashMap<SessionId, Arc<tokio::sync::Mutex<()>>>>,
    capabilities: Arc<SessionRuntimeServices>,
    event_bus: OnceLock<Arc<ServerEventBus>>,
    resource_cleanups: Mutex<Vec<Arc<dyn SessionResourceCleanup>>>,
}

impl SessionManager {
    // ─── 生命周期 ─────────────────────────────────────────────────────

    pub fn new(
        event_store: Arc<dyn EventStore>,
        config: Arc<ConfigManager>,
        capabilities: Arc<SessionRuntimeServices>,
        resource_cleanups: Vec<Arc<dyn SessionResourceCleanup>>,
    ) -> Self {
        Self {
            event_store,
            config,
            runtime_states: Mutex::new(HashMap::new()),
            open_locks: Mutex::new(HashMap::new()),
            capabilities,
            event_bus: OnceLock::new(),
            resource_cleanups: Mutex::new(resource_cleanups),
        }
    }

    /// 绑定事件总线。SessionManager 在 create/fork/open 返回 session 时自动 attach，
    /// 在 delete/recycle 时自动 detach，确保 session 事件流始终与广播通道连通。
    pub fn bind_event_bus(&self, event_bus: Arc<ServerEventBus>) {
        // 幂等：如果已设置则静默忽略。
        let _ = self.event_bus.set(event_bus);
    }

    /// 添加资源清理回调。
    pub fn add_resource_cleanup(&self, cleanup: Arc<dyn SessionResourceCleanup>) {
        self.resource_cleanups.lock().push(cleanup);
    }

    fn get_or_create_runtime(&self, session_id: &SessionId) -> Arc<SessionRuntimeState> {
        self.get_or_create_runtime_with_state(session_id).0
    }

    fn get_or_create_runtime_with_state(
        &self,
        session_id: &SessionId,
    ) -> (Arc<SessionRuntimeState>, bool) {
        let mut runtime_states = self.runtime_states.lock();
        if let Some(runtime) = runtime_states.get(session_id) {
            return (Arc::clone(runtime), false);
        }
        let model_id = self.config.read_effective().llm.model_id.clone();
        let runtime = Arc::new(SessionRuntimeState::new(
            self.capabilities.llm(),
            self.capabilities.small_llm(),
            model_id,
        ));
        runtime_states.insert(session_id.clone(), Arc::clone(&runtime));
        (runtime, true)
    }

    fn remove_runtime_if_same(&self, session_id: &SessionId, expected: &Arc<SessionRuntimeState>) {
        let mut runtime_states = self.runtime_states.lock();
        if runtime_states
            .get(session_id)
            .is_some_and(|runtime| Arc::ptr_eq(runtime, expected))
        {
            runtime_states.remove(session_id);
        }
    }

    fn open_lock(&self, session_id: &SessionId) -> Arc<tokio::sync::Mutex<()>> {
        self.open_locks
            .lock()
            .entry(session_id.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    fn remove_open_lock_if_idle(
        &self,
        session_id: &SessionId,
        expected: &Arc<tokio::sync::Mutex<()>>,
    ) {
        let mut open_locks = self.open_locks.lock();
        if open_locks
            .get(session_id)
            .is_some_and(|lock| Arc::ptr_eq(lock, expected))
            && Arc::strong_count(expected) == 2
        {
            open_locks.remove(session_id);
        }
    }

    pub(crate) fn config(&self) -> &Arc<ConfigManager> {
        &self.config
    }

    pub(crate) fn event_store(&self) -> &Arc<dyn EventStore> {
        &self.event_store
    }

    /// 把子会话的 runtime 注册到 manager，并 attach 到 event bus（与 create/open/fork 一致）。
    ///
    /// 子会话由 `Session::spawn_child` 创建，其 runtime 不经过 `get_or_create_runtime`，
    /// 必须手动注册才能让后续 `open(child_sid)` 拿到同一个 runtime（共享广播通道）。
    pub(crate) fn register_child_session(&self, session: &Session) {
        let sid = session.id().clone();
        let runtime = session.runtime().clone();
        self.runtime_states.lock().insert(sid, runtime);
        self.attach_session_to_event_bus(session);
    }

    fn attach_session_to_event_bus(&self, session: &Session) {
        if let Some(bus) = self.event_bus.get() {
            bus.attach(session);
        }
    }

    /// 让所有已打开 session 的工具快照失效；下一次 turn 会按当前扩展集重建。
    pub(crate) fn invalidate_tool_registries(&self) {
        for runtime in self.runtime_states.lock().values() {
            runtime.set_tool_registry(Arc::new(ToolRegistry::new()));
        }
    }

    pub(crate) async fn create(
        &self,
        working_dir: &str,
    ) -> Result<CreatedSession, SessionManagerError> {
        let model_id = self.config.read_effective().llm.model_id.clone();
        // 先在 registry 里登记 runtime，再创建 Session 让两者共享同一份。
        let sid = astrcode_core::types::new_session_id();
        let runtime = self.get_or_create_runtime(&sid);
        let session = Session::create_with_params(SessionCreateParams {
            store: Arc::clone(&self.event_store),
            sid: sid.clone(),
            working_dir: working_dir.to_string(),
            model_id,
            parent: None,
            tool_policy: None,
            source_extension: None,
            runtime,
            caps: Arc::clone(&self.capabilities),
        })
        .await?;

        self.attach_session_to_event_bus(&session);

        let start_event = self
            .event_store
            .replay_events(&sid)
            .await?
            .into_iter()
            .next()
            .ok_or(SessionManagerError::MissingStartEvent)?;

        session.emit_lifecycle(ExtensionEvent::SessionStart).await?;

        Ok(CreatedSession {
            session,
            start_event,
        })
    }

    pub(crate) async fn open(&self, session_id: SessionId) -> Result<Session, SessionManagerError> {
        let open_lock = self.open_lock(&session_id);
        let opening = open_lock.lock().await;
        let result = async {
            let (runtime, resumed) = self.get_or_create_runtime_with_state(&session_id);
            let session = match Session::open(
                Arc::clone(&self.event_store),
                session_id.clone(),
                Arc::clone(&runtime),
                Arc::clone(&self.capabilities),
            )
            .await
            {
                Ok(session) => session,
                Err(error) => {
                    if resumed {
                        self.remove_runtime_if_same(&session_id, &runtime);
                    }
                    return Err(error.into());
                },
            };
            if resumed {
                if let Err(error) = session.emit_lifecycle(ExtensionEvent::SessionResume).await {
                    self.remove_runtime_if_same(&session_id, &runtime);
                    return Err(error.into());
                }
            }
            self.attach_session_to_event_bus(&session);
            Ok(session)
        }
        .await;
        drop(opening);
        self.remove_open_lock_if_idle(&session_id, &open_lock);
        result
    }

    /// 停止活跃 turn、清空待处理队列后删除 durable session。
    pub(crate) async fn delete_with_turn_teardown(
        &self,
        scheduler: &TurnScheduler,
        session_id: &SessionId,
    ) -> Result<(), SessionManagerError> {
        scheduler.teardown_session(session_id).await;
        self.delete(session_id).await
    }

    pub(crate) async fn delete(&self, session_id: &SessionId) -> Result<(), SessionManagerError> {
        let model = self.event_store.session_read_model(session_id).await?;
        emit_lifecycle_for_read_model(
            &self.capabilities,
            session_id,
            &model,
            ExtensionEvent::SessionShutdown,
        )
        .await?;
        self.event_store.delete_session(session_id).await?;
        self.cleanup_session_resources(session_id);
        // 清理本 session 关联的持久化终端。
        // 已通过 SessionResourceCleanup trait 注入，见 TerminalCleanup。
        Ok(())
    }

    /// 追加 durable 事件；若进程内 runtime 仍存在则 fan-out，不触发 `open` / `SessionResume`。
    pub(crate) async fn append_durable_event(
        &self,
        session_id: &SessionId,
        event: Event,
    ) -> Result<Event, SessionManagerError> {
        let stored = self.event_store.append_event(event).await?;
        if let Some(runtime) = self.runtime_states.lock().get(session_id) {
            runtime.fanout_persisted(stored.clone());
        }
        Ok(stored)
    }

    /// 释放 session 占用的进程内资源（delete / recycle 共用）。
    fn cleanup_session_resources(&self, session_id: &SessionId) {
        if let Some(runtime) = self.runtime_states.lock().remove(session_id) {
            runtime
                .background_tasks()
                .lock()
                .cleanup_session(session_id);
        }
        if let Some(bus) = self.event_bus.get() {
            bus.detach(session_id);
        }
        for cleanup in self.resource_cleanups.lock().iter() {
            cleanup.cleanup(session_id);
        }
    }

    /// 强制 fsync 指定会话的 durable event log。
    pub(crate) async fn sync_durable_events(&self, session_id: &SessionId) {
        if let Err(e) = self.event_store.sync_durable_events(session_id).await {
            tracing::error!(session_id = %session_id, error = %e, "failed to sync durable events");
        }
    }

    pub(crate) async fn recycle_session(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionManagerError> {
        let model = self.event_store.session_read_model(session_id).await?;
        emit_lifecycle_for_read_model(
            &self.capabilities,
            session_id,
            &model,
            ExtensionEvent::SessionShutdown,
        )
        .await?;
        self.event_store
            .recycle_session(session_id)
            .await
            .map_err(SessionManagerError::from)?;
        self.cleanup_session_resources(session_id);
        Ok(())
    }

    /// 从 .recycled/ 恢复一个已回收的 session。
    pub(crate) async fn restore_session(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionManagerError> {
        self.event_store
            .restore_session(session_id)
            .await
            .map_err(SessionManagerError::from)
    }

    /// Fork 一个已有会话，创建新 session 并复制 fork 点之前的消息前缀。
    ///
    /// fork 保证新 session 发送给 LLM 的 system prompt + 消息前缀与源 session 完全一致，
    /// 从而让 provider 侧的 KV 缓存（prompt cache）自动命中。
    ///
    /// - `source_id`: 源会话 ID
    /// - `at_cursor`: 可选 fork 点 cursor（event seq 的十进制字符串），为 None 则从末尾 fork
    ///
    /// 返回新 session 及其初始事件。
    pub(crate) async fn fork(
        &self,
        source_id: &SessionId,
        at_cursor: Option<&Cursor>,
    ) -> Result<ForkedSession, SessionManagerError> {
        // 1. 读源 session 的 read model
        let source_model = self.event_store.session_read_model(source_id).await?;

        // 2. 确定 fork 点 cursor
        let fork_cursor = match at_cursor {
            Some(cursor) => cursor.clone(),
            None => source_model.cursor(),
        };

        // 3. 计算 fork 点之前的 provider 消息 如果 at_cursor 为 None（从末尾 fork），直接用 read
        //    model 的消息。 如果指定了 cursor，需要从事件日志重放到指定点来获取消息。
        let (context_messages, retained_messages) = if at_cursor.is_some() {
            let truncated_seq: u64 = fork_cursor
                .parse()
                .map_err(|_| SessionManagerError::InvalidCursor(fork_cursor.clone()))?;
            let truncated_events = self
                .event_store
                .replay_events_through(source_id, truncated_seq)
                .await?;
            let truncated_model =
                astrcode_storage::projection::replay(source_id.clone(), &truncated_events);
            (truncated_model.context_messages, truncated_model.messages)
        } else {
            (
                source_model.context_messages.clone(),
                source_model.messages.clone(),
            )
        };

        // 4. 创建新 session
        let model_id = self.config.read_effective().llm.model_id.clone();
        let new_sid = astrcode_core::types::new_session_id();
        let runtime = self.get_or_create_runtime(&new_sid);
        let session = Session::create_with_params(SessionCreateParams {
            store: Arc::clone(&self.event_store),
            sid: new_sid.clone(),
            working_dir: source_model.working_dir.clone(),
            model_id,
            parent: None,
            tool_policy: None,
            source_extension: None,
            runtime,
            caps: Arc::clone(&self.capabilities),
        })
        .await?;

        self.attach_session_to_event_bus(&session);

        // 5. 写入 SessionForked 事件（attach 之后 append，经 fanout 自动进入 event bus）
        session
            .append_event(Event::new(
                new_sid.clone(),
                None,
                EventPayload::SessionForked {
                    source_session_id: source_id.clone(),
                    source_cursor: fork_cursor,
                    context_messages: context_messages.into_iter().map(|m| m.message).collect(),
                    retained_messages: retained_messages.into_iter().map(|m| m.message).collect(),
                },
            ))
            .await?;

        // 6. 复制源 session 的 system prompt 配置到新 session（保证 KV 前缀一致）
        if let (Some(text), Some(fingerprint)) = (
            &source_model.system_prompt,
            &source_model.system_prompt_fingerprint,
        ) {
            session
                .append_event(Event::new(
                    new_sid.clone(),
                    None,
                    EventPayload::SystemPromptConfigured {
                        text: text.clone(),
                        fingerprint: fingerprint.clone(),
                        extra_system_prompt: source_model.extra_system_prompt.clone(),
                    },
                ))
                .await?;
        }

        Ok(ForkedSession { session })
    }
}

#[cfg(test)]
mod runtime_registry_tests {
    use std::sync::Arc;

    use astrcode_core::{
        config::{
            AgentSettings, ContextSettings, EffectiveConfig, ExtensionSettings, LlmSettings,
            OpenAiApiMode,
        },
        extension::ChildToolPolicy,
        llm::{LlmError, LlmEvent, LlmMessage, LlmProvider, ModelLimits},
        storage::EventStore,
        tool::ToolDefinition,
        types::SessionId,
    };
    use astrcode_session::{SessionCreateParams, SessionRuntimeState};
    use tokio::sync::mpsc;

    use super::*;

    struct NoopLlm;

    #[async_trait::async_trait]
    impl LlmProvider for NoopLlm {
        async fn generate(
            &self,
            _messages: Vec<LlmMessage>,
            _tools: Vec<ToolDefinition>,
        ) -> Result<mpsc::UnboundedReceiver<LlmEvent>, LlmError> {
            unreachable!()
        }

        fn model_limits(&self) -> ModelLimits {
            ModelLimits {
                max_input_tokens: 1,
                max_output_tokens: 1,
            }
        }
    }

    fn test_manager() -> (SessionManager, Arc<dyn EventStore>) {
        let store: Arc<dyn EventStore> =
            Arc::new(astrcode_storage::in_memory::InMemoryEventStore::new());
        let runner = Arc::new(astrcode_extensions::runner::ExtensionRunner::new(
            std::time::Duration::from_secs(1),
        ));
        let caps = Arc::new(SessionRuntimeServices::new(
            Arc::new(NoopLlm),
            Arc::new(NoopLlm),
            Arc::clone(&runner),
            Arc::new(
                astrcode_context::context_assembler::LlmContextAssembler::new(
                    ContextSettings::default(),
                ),
            ),
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
                context: ContextSettings::default(),
                agent: AgentSettings::default(),
                extensions: ExtensionSettings::default(),
            },
        ));
        let config = Arc::new(ConfigManager::new(
            Arc::new(astrcode_storage::config_store::FileConfigStore::new(
                std::path::PathBuf::from("target/session-manager-registry-test-config.json"),
            )),
            astrcode_core::config::Config::default(),
            Arc::clone(&caps),
        ));
        let manager = SessionManager::new(store.clone(), config, caps, vec![]);
        (manager, store)
    }

    #[tokio::test]
    async fn register_child_session_preserves_spawned_runtime() {
        let (manager, store) = test_manager();
        let parent = manager.create(".").await.unwrap().session;
        let deny_policy = ChildToolPolicy::Deny {
            tools: vec!["agent".into()],
        };
        let child_runtime = Arc::new(SessionRuntimeState::new(
            Arc::new(NoopLlm),
            Arc::new(NoopLlm),
            "mock".into(),
        ));
        child_runtime.set_tool_policy(Some(deny_policy.clone()));

        let child = Session::create_with_params(SessionCreateParams {
            store: Arc::clone(&store),
            sid: SessionId::from("child-1"),
            working_dir: ".".into(),
            model_id: "mock".into(),
            parent: Some(parent.id().clone()),
            tool_policy: Some(deny_policy),
            source_extension: None,
            runtime: Arc::clone(&child_runtime),
            caps: Arc::clone(parent.caps()),
        })
        .await
        .unwrap();
        manager.register_child_session(&child);

        let reopened = manager.open(SessionId::from("child-1")).await.unwrap();
        assert!(Arc::ptr_eq(reopened.runtime(), &child_runtime));
        assert_eq!(
            reopened.runtime().tool_policy(),
            Some(ChildToolPolicy::Deny {
                tools: vec!["agent".into()],
            })
        );
    }

    #[tokio::test]
    async fn open_without_register_replaces_child_runtime() {
        let (manager, store) = test_manager();
        let parent = manager.create(".").await.unwrap().session;
        let child_runtime = Arc::new(SessionRuntimeState::new(
            Arc::new(NoopLlm),
            Arc::new(NoopLlm),
            "mock".into(),
        ));
        child_runtime.set_tool_policy(Some(ChildToolPolicy::Deny {
            tools: vec!["agent".into()],
        }));

        Session::create_with_params(SessionCreateParams {
            store: Arc::clone(&store),
            sid: SessionId::from("child-2"),
            working_dir: ".".into(),
            model_id: "mock".into(),
            parent: Some(parent.id().clone()),
            tool_policy: Some(ChildToolPolicy::Deny {
                tools: vec!["agent".into()],
            }),
            source_extension: None,
            runtime: Arc::clone(&child_runtime),
            caps: Arc::clone(parent.caps()),
        })
        .await
        .unwrap();

        let reopened = manager.open(SessionId::from("child-2")).await.unwrap();
        assert!(!Arc::ptr_eq(reopened.runtime(), &child_runtime));
        // `Session::open` 会从 durable read model 恢复 tool_policy，但 runtime 实例仍是新建的。
        assert_eq!(
            reopened.runtime().tool_policy(),
            Some(ChildToolPolicy::Deny {
                tools: vec!["agent".into()],
            })
        );
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc, time::Duration};

    use astrcode_context::prompt_engine::load_system_prompt_files;
    use astrcode_core::{
        extension::{Extension, Registrar, ToolHandler},
        tool::{ExecutionMode, ToolDefinition, ToolOrigin, ToolResult},
    };
    use astrcode_extensions::runner::ExtensionRunner;
    use astrcode_session::session_setup::{
        SystemPromptSnapshotInput, build_system_prompt_snapshot, build_tool_registry_snapshot,
    };

    struct StaticToolExtension {
        id: &'static str,
        tool_name: &'static str,
        description: &'static str,
    }

    #[async_trait::async_trait]
    impl Extension for StaticToolExtension {
        fn id(&self) -> &str {
            self.id
        }

        fn register(&self, reg: &mut Registrar) {
            reg.tool(
                ToolDefinition {
                    name: self.tool_name.into(),
                    description: self.description.into(),
                    parameters: serde_json::json!({"type": "object"}),
                    origin: ToolOrigin::Extension,
                    execution_mode: ExecutionMode::Sequential,
                },
                Arc::new(StaticToolHandler),
            );
        }
    }

    struct StaticToolHandler;

    #[async_trait::async_trait]
    impl ToolHandler for StaticToolHandler {
        async fn execute(
            &self,
            tool_name: &str,
            _arguments: serde_json::Value,
            _working_dir: &str,
            _ctx: &astrcode_core::tool::ToolExecutionContext,
        ) -> Result<ToolResult, astrcode_core::extension::ExtensionError> {
            Err(astrcode_core::extension::ExtensionError::NotFound(
                tool_name.into(),
            ))
        }
    }

    #[tokio::test]
    async fn child_extra_system_prompt_participates_in_snapshot_build() {
        let runner = ExtensionRunner::new(Duration::from_secs(1));
        let prompt_files = load_system_prompt_files(".").await;
        let (system_prompt, fingerprint) =
            build_system_prompt_snapshot(SystemPromptSnapshotInput {
                extension_runner: &runner,
                session_id: "session-1",
                working_dir: ".",
                model_id: "mock",
                tools: &[],
                extra_system_prompt: Some("child body"),
                tool_prompt_metadata: HashMap::new(),
                prompt_files,
            })
            .await
            .unwrap();

        assert!(system_prompt.contains("child body"));
        assert!(!fingerprint.is_empty());
    }

    #[tokio::test]
    async fn tool_snapshot_precedence_is_explicit() {
        let runner = ExtensionRunner::new(Duration::from_secs(1));
        runner
            .register(Arc::new(StaticToolExtension {
                id: "first",
                tool_name: "shell",
                description: "first extension shell",
            }))
            .await
            .unwrap();
        runner
            .register(Arc::new(StaticToolExtension {
                id: "second",
                tool_name: "shell",
                description: "second extension shell",
            }))
            .await
            .unwrap();

        let registry = build_tool_registry_snapshot(&runner, ".", 1, None).await;
        let shell = registry.find_definition("shell").unwrap();

        assert_eq!(shell.origin, ToolOrigin::Extension);
        assert_eq!(shell.description, "first extension shell");
    }
}

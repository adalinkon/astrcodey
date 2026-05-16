use std::{collections::HashMap, sync::Arc};

use astrcode_context::prompt_engine::{PromptEngine, PromptFiles, load_system_prompt_files};
use astrcode_core::{
    config::ModelSelection,
    event::{Event, EventPayload},
    extension::{ExtensionError, ExtensionEvent, PromptBuildContext},
    prompt::{ExtensionPromptBlock, ExtensionSection, PromptProvider, SystemPromptInput},
    storage::{EventStore, SessionReadModel, SessionSummary, StorageError},
    tool::{FileObservationStore, ToolDefinition, ToolPromptMetadata},
    types::{Cursor, SessionId},
};
use astrcode_extensions::runner::ExtensionRunner;
use astrcode_session::{
    Session, SessionError, SessionRuntimeRegistry, background::BackgroundTaskManager,
};
use astrcode_support::{hash::hex_fingerprint, shell::resolve_shell};
use astrcode_tools::registry::{ToolRegistry, builtin_tools};
use parking_lot::Mutex;

use crate::config_manager::ConfigManager;

pub(crate) struct CreatedSession {
    pub(crate) session: Session,
    pub(crate) start_event: Event,
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
}

struct SystemPromptSnapshotInput<'a> {
    extension_runner: &'a ExtensionRunner,
    session_id: &'a str,
    working_dir: &'a str,
    model_id: &'a str,
    tools: &'a [ToolDefinition],
    extra_system_prompt: Option<&'a str>,
    tool_prompt_metadata: HashMap<String, ToolPromptMetadata>,
    prompt_files: PromptFiles,
}

/// Server 侧的 session 生命周期门面。
///
/// durable session 仍由 [`Session`] / [`EventStore`] 负责；这里集中管理
/// 与 session 同生灭的进程内资源，避免 handler 逐项记忆清理细节。
pub struct SessionManager {
    event_store: Arc<dyn EventStore>,
    config: Arc<ConfigManager>,
    extension_runner: Arc<ExtensionRunner>,
    runtime_registry: Arc<SessionRuntimeRegistry>,
    background_tasks: Arc<Mutex<BackgroundTaskManager>>,
    tool_registries: Mutex<HashMap<SessionId, Arc<ToolRegistry>>>,
}

impl SessionManager {
    // ─── 生命周期 ─────────────────────────────────────────────────────

    pub fn new(
        event_store: Arc<dyn EventStore>,
        config: Arc<ConfigManager>,
        extension_runner: Arc<ExtensionRunner>,
        runtime_registry: Arc<SessionRuntimeRegistry>,
        background_tasks: Arc<Mutex<BackgroundTaskManager>>,
    ) -> Self {
        Self {
            event_store,
            config,
            extension_runner,
            runtime_registry,
            background_tasks,
            tool_registries: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) async fn create(
        &self,
        working_dir: &str,
    ) -> Result<CreatedSession, SessionManagerError> {
        let model_id = self.config.read_effective().llm.model_id.clone();
        let session =
            Session::create(Arc::clone(&self.event_store), working_dir, &model_id, None).await?;
        let sid = session.id().clone();
        self.runtime_registry.get_or_create(&sid);

        let start_event = self
            .event_store
            .replay_events(&sid)
            .await?
            .into_iter()
            .next()
            .ok_or(SessionManagerError::MissingStartEvent)?;

        let lifecycle_ctx = astrcode_core::extension::LifecycleContext {
            session_id: sid.to_string(),
            working_dir: working_dir.to_string(),
            model: ModelSelection::simple(model_id),
        };
        self.extension_runner
            .emit_lifecycle(ExtensionEvent::SessionStart, lifecycle_ctx)
            .await?;

        Ok(CreatedSession {
            session,
            start_event,
        })
    }

    pub(crate) async fn open(&self, session_id: SessionId) -> Result<Session, SessionManagerError> {
        let session = Session::open(Arc::clone(&self.event_store), session_id.clone()).await?;
        self.runtime_registry.get_or_create(&session_id);
        Ok(session)
    }

    pub(crate) async fn create_child(
        &self,
        working_dir: &str,
        model_id: &str,
        parent_session_id: &SessionId,
    ) -> Result<Session, SessionManagerError> {
        let session = Session::create(
            Arc::clone(&self.event_store),
            working_dir,
            model_id,
            Some(parent_session_id),
        )
        .await?;
        self.runtime_registry.get_or_create(session.id());
        Ok(session)
    }

    pub(crate) async fn delete(&self, session_id: &SessionId) -> Result<(), SessionManagerError> {
        let lifecycle_ctx = astrcode_core::extension::LifecycleContext {
            session_id: session_id.to_string(),
            working_dir: String::new(),
            model: ModelSelection::simple(self.config.read_effective().llm.model_id.clone()),
        };
        self.extension_runner
            .emit_lifecycle(ExtensionEvent::SessionShutdown, lifecycle_ctx)
            .await?;
        self.event_store.delete_session(session_id).await?;
        self.cleanup_background_tasks(session_id);
        self.runtime_registry.remove(session_id);
        self.tool_registries.lock().remove(session_id);
        Ok(())
    }

    // ─── 只读查询 ─────────────────────────────────────────────────────

    pub(crate) async fn read_model(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionReadModel, SessionManagerError> {
        self.event_store
            .session_read_model(session_id)
            .await
            .map_err(SessionManagerError::from)
    }

    pub(crate) async fn list_summaries(&self) -> Result<Vec<SessionSummary>, SessionManagerError> {
        self.event_store
            .list_session_summaries()
            .await
            .map_err(SessionManagerError::from)
    }

    pub(crate) async fn replay_from(
        &self,
        session_id: &SessionId,
        cursor: &Cursor,
    ) -> Result<Vec<Event>, SessionManagerError> {
        self.event_store
            .replay_from(session_id, cursor)
            .await
            .map_err(SessionManagerError::from)
    }

    pub(crate) async fn latest_cursor(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Cursor>, SessionManagerError> {
        self.event_store
            .latest_cursor(session_id)
            .await
            .map_err(SessionManagerError::from)
    }

    // ─── session 级运行时资源 ─────────────────────────────────────────

    pub(crate) async fn ensure_tool_registry(
        &self,
        session_id: &SessionId,
        working_dir: &str,
    ) -> Arc<ToolRegistry> {
        if let Some(registry) = self.tool_registries.lock().get(session_id).cloned() {
            return registry;
        }

        self.refresh_tool_registry(session_id, working_dir).await
    }

    pub(crate) async fn refresh_tool_registry(
        &self,
        session_id: &SessionId,
        working_dir: &str,
    ) -> Arc<ToolRegistry> {
        let timeout = self.config.read_effective().llm.read_timeout_secs;
        let registry =
            build_tool_registry_snapshot(&self.extension_runner, working_dir, timeout).await;
        self.tool_registries
            .lock()
            .insert(session_id.clone(), Arc::clone(&registry));
        registry
    }

    pub(crate) fn file_observation_store(
        &self,
        session_id: &SessionId,
    ) -> Arc<dyn FileObservationStore> {
        self.runtime_registry
            .get_or_create(session_id)
            .file_observation_store()
    }

    pub(crate) fn cleanup_background_tasks(&self, session_id: &SessionId) {
        self.background_tasks.lock().cleanup_session(session_id);
    }

    // ─── prompt 初始化 ────────────────────────────────────────────────

    pub(crate) async fn initialize_system_prompt(
        &self,
        session_id: &SessionId,
        working_dir: &str,
        extra_system_prompt: Option<&str>,
    ) -> Result<(Arc<ToolRegistry>, Event), SessionManagerError> {
        let registry_fut = self.refresh_tool_registry(session_id, working_dir);
        let prompt_files_fut = load_system_prompt_files(working_dir);
        let (tool_registry, prompt_files) = tokio::join!(registry_fut, prompt_files_fut);
        let event = self
            .configure_system_prompt_with_files(
                session_id,
                working_dir,
                &tool_registry,
                extra_system_prompt,
                prompt_files,
            )
            .await?;
        Ok((tool_registry, event))
    }

    pub(crate) async fn configure_system_prompt(
        &self,
        session_id: &SessionId,
        working_dir: &str,
        tool_registry: &ToolRegistry,
        extra_system_prompt: Option<&str>,
    ) -> Result<Event, SessionManagerError> {
        let prompt_files = load_system_prompt_files(working_dir).await;
        self.configure_system_prompt_with_files(
            session_id,
            working_dir,
            tool_registry,
            extra_system_prompt,
            prompt_files,
        )
        .await
    }

    pub(crate) async fn build_system_prompt_snapshot(
        &self,
        session_id: &SessionId,
        working_dir: &str,
        model_id: &str,
        tool_registry: &ToolRegistry,
        extra_system_prompt: Option<&str>,
    ) -> Result<(String, String), SessionManagerError> {
        let prompt_files = load_system_prompt_files(working_dir).await;
        self.build_system_prompt_snapshot_with_files(
            session_id,
            working_dir,
            model_id,
            tool_registry,
            extra_system_prompt,
            prompt_files,
        )
        .await
    }

    async fn configure_system_prompt_with_files(
        &self,
        session_id: &SessionId,
        working_dir: &str,
        tool_registry: &ToolRegistry,
        extra_system_prompt: Option<&str>,
        prompt_files: PromptFiles,
    ) -> Result<Event, SessionManagerError> {
        let model_id = self.config.read_effective().llm.model_id.clone();
        let (system_prompt, fingerprint) = self
            .build_system_prompt_snapshot_with_files(
                session_id,
                working_dir,
                &model_id,
                tool_registry,
                extra_system_prompt,
                prompt_files,
            )
            .await?;
        self.event_store
            .append_event(Event::new(
                session_id.clone(),
                None,
                EventPayload::SystemPromptConfigured {
                    text: system_prompt,
                    fingerprint,
                },
            ))
            .await
            .map_err(SessionManagerError::from)
    }

    async fn build_system_prompt_snapshot_with_files(
        &self,
        session_id: &SessionId,
        working_dir: &str,
        model_id: &str,
        tool_registry: &ToolRegistry,
        extra_system_prompt: Option<&str>,
        prompt_files: PromptFiles,
    ) -> Result<(String, String), SessionManagerError> {
        let tools_with_meta = tool_registry.list_definitions_with_prompt_metadata();
        let tools: Vec<_> = tools_with_meta.iter().map(|(def, _)| def.clone()).collect();
        let tool_prompt_metadata = tools_with_meta
            .into_iter()
            .filter_map(|(def, meta)| meta.map(|m| (def.name, m)))
            .collect();
        build_system_prompt_snapshot_with_files(SystemPromptSnapshotInput {
            extension_runner: &self.extension_runner,
            session_id: session_id.as_str(),
            working_dir,
            model_id,
            tools: &tools,
            extra_system_prompt,
            tool_prompt_metadata,
            prompt_files,
        })
        .await
        .map_err(SessionManagerError::from)
    }
}

/// 构建一个工作目录绑定的工具表快照。
///
/// 每次新建/恢复 session 时调用一次；工具执行期间只读取这份快照，
/// 不再维护运行中的动态工具层。
async fn build_tool_registry_snapshot(
    extension_runner: &ExtensionRunner,
    working_dir: &str,
    timeout_secs: u64,
) -> Arc<ToolRegistry> {
    let mut tool_registry = ToolRegistry::new();

    for tool in builtin_tools(std::path::PathBuf::from(working_dir), timeout_secs) {
        tool_registry.register(tool);
    }

    // Extensions override builtins, and earlier registered extensions keep
    // precedence over later registered extensions with the same tool name.
    for tool in extension_runner
        .collect_tool_adapters_typed(working_dir)
        .await
        .into_iter()
        .rev()
    {
        tool_registry.register(tool);
    }

    Arc::new(tool_registry)
}

async fn build_system_prompt_snapshot_with_files(
    input: SystemPromptSnapshotInput<'_>,
) -> Result<(String, String), ExtensionError> {
    let SystemPromptSnapshotInput {
        extension_runner,
        session_id,
        working_dir,
        model_id,
        tools,
        extra_system_prompt,
        tool_prompt_metadata,
        prompt_files,
    } = input;

    let prompt_ctx = PromptBuildContext {
        session_id: session_id.to_string(),
        working_dir: working_dir.to_string(),
        model: ModelSelection::simple(model_id),
        tools: tools.to_vec(),
    };

    let contributions = extension_runner
        .collect_prompt_contributions_typed(prompt_ctx)
        .await?;

    let mut extension_blocks = Vec::new();
    for content in contributions.system_prompts {
        extension_blocks.push(ExtensionPromptBlock {
            section: ExtensionSection::PlatformInstructions,
            content,
        });
    }
    for content in contributions.additional_instructions {
        extension_blocks.push(ExtensionPromptBlock {
            section: ExtensionSection::AdditionalInstructions,
            content,
        });
    }
    for content in contributions.skills {
        extension_blocks.push(ExtensionPromptBlock {
            section: ExtensionSection::Skills,
            content,
        });
    }
    for content in contributions.agents {
        extension_blocks.push(ExtensionPromptBlock {
            section: ExtensionSection::Agents,
            content,
        });
    }
    let extra_instructions = extra_system_prompt.and_then(|s| {
        let trimmed = s.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    });

    let mut merged_metadata = tool_prompt_metadata;
    merged_metadata.extend(extension_runner.collect_tool_prompt_metadata_typed().await);

    let input = SystemPromptInput {
        working_dir: working_dir.to_string(),
        os: std::env::consts::OS.into(),
        shell: resolve_shell().name,
        date: chrono::Utc::now().format("%Y-%m-%d").to_string(),
        identity: prompt_files.identity,
        user_rules: prompt_files.user_rules,
        project_rules: prompt_files.project_rules,
        tools: tools.to_vec(),
        tool_prompt_metadata: merged_metadata,
        extension_blocks,
        extra_instructions,
    };

    let system_prompt = PromptEngine::new()
        .assemble(input)
        .await
        .system_prompt
        .unwrap_or_default();
    let fingerprint = hex_fingerprint(system_prompt.as_bytes());
    Ok((system_prompt, fingerprint))
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use astrcode_core::{
        extension::{Extension, Registrar, ToolHandler},
        tool::{ExecutionMode, ToolDefinition, ToolOrigin, ToolResult},
    };

    use super::*;

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
            build_system_prompt_snapshot_with_files(SystemPromptSnapshotInput {
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
            .await;
        runner
            .register(Arc::new(StaticToolExtension {
                id: "second",
                tool_name: "shell",
                description: "second extension shell",
            }))
            .await;

        let registry = build_tool_registry_snapshot(&runner, ".", 1).await;
        let shell = registry.find_definition("shell").unwrap();

        assert_eq!(shell.origin, ToolOrigin::Extension);
        assert_eq!(shell.description, "first extension shell");
    }
}

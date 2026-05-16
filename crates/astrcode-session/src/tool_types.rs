//! Agent 工具调用的数据类型定义。
//!
//! 包含工具调用从 LLM 流式响应中积累、预处理、到最终执行各阶段的类型。

use std::collections::BTreeMap;

use astrcode_core::{
    llm::LlmMessage,
    storage::ToolResultArtifactReader,
    tool::{AgentSessionControl, BackgroundTaskReader, ExecutionMode, ToolDefinition, ToolResult},
    types::*,
};
use tokio::sync::mpsc;

use super::{background::BackgroundTaskManager, turn_context::AgentSignal};

/// 等待执行的工具调用，在 LLM 流式响应中逐步积累参数。
pub struct PendingToolCall {
    /// 工具调用的唯一标识
    pub call_id: String,
    /// 工具名称
    pub name: String,
    /// 工具调用的 JSON 参数（可能跨多个 delta 事件拼接）
    pub arguments: String,
}

pub struct PreparedToolCall {
    pub index: usize,
    pub call_id: String,
    pub name: String,
    pub tool_input: serde_json::Value,
    pub mode: ExecutionMode,
    pub outcome: PreparedToolOutcome,
}

pub struct ExecuteToolCalls<'a> {
    pub prepared: &'a [PreparedToolCall],
    pub tools: &'a [ToolDefinition],
    pub messages: &'a mut Vec<LlmMessage>,
    pub all_tool_results: &'a mut Vec<ToolResult>,
    pub event_tx: &'a Option<mpsc::UnboundedSender<AgentSignal>>,
}

pub struct CommitToolResults<'a> {
    pub prepared: &'a [PreparedToolCall],
    pub results: BTreeMap<usize, ToolResult>,
    pub messages: &'a mut Vec<LlmMessage>,
    pub all_tool_results: &'a mut Vec<ToolResult>,
    pub event_tx: &'a Option<mpsc::UnboundedSender<AgentSignal>>,
}

pub struct PendingCommittedToolResult {
    pub call_id: String,
    pub tool_name: String,
    pub result: ToolResult,
}

pub enum ToolExecutionStep {
    Blocked(ToolResult),
    Parallel(ExecutableToolCall),
    Sequential(ExecutableToolCall),
}

pub enum PreparedToolOutcome {
    Ready,
    Blocked(ToolResult),
}

#[derive(Clone)]
pub struct ExecutableToolCall {
    pub index: usize,
    pub call_id: String,
    pub name: String,
    pub tool_input: serde_json::Value,
}

pub struct ToolCallRuntimeContext {
    pub session_id: SessionId,
    pub working_dir: String,
    pub model_id: String,
    pub tools: Vec<ToolDefinition>,
    pub tool_result_reader: Option<Arc<dyn ToolResultArtifactReader>>,
    pub event_tx: Option<mpsc::UnboundedSender<AgentSignal>>,
    pub capabilities: ToolRuntimeCapabilities,
}

impl PreparedToolCall {
    /// 将预处理后的工具调用转换为可执行任务输入。
    pub fn to_executable(&self) -> ExecutableToolCall {
        ExecutableToolCall {
            index: self.index,
            call_id: self.call_id.clone(),
            name: self.name.clone(),
            tool_input: self.tool_input.clone(),
        }
    }
}

use std::sync::Arc;

// ─── Tool runtime capabilities ──────────────────────────────────────────

/// 会话级工具运行时能力，从 ToolPipeline 透传到 ToolExecutionContext。
///
/// 整合了后台任务、文件观察、agent 会话控制等按 session 生命周期存在的能力。
#[derive(Clone)]
pub struct ToolRuntimeCapabilities {
    /// 后台任务完成后的通知通道。
    pub background_result_tx:
        Option<mpsc::UnboundedSender<crate::background::BackgroundTaskCompletion>>,
    /// 后台任务管理器，用于注册 watcher handle 以支持取消。
    pub background_tasks: Arc<parking_lot::Mutex<BackgroundTaskManager>>,
    /// 后台任务只读接口，注入到 ToolExecutionContext 供 TaskTool 使用。
    pub background_task_reader: Option<Arc<dyn BackgroundTaskReader>>,
    /// 文件观察存储，用于 read/edit 协作的 read-before-edit 守卫。
    pub file_observation_store: Option<Arc<dyn astrcode_core::tool::FileObservationStore>>,
    /// Agent 会话操控能力，用于 send 等工具与子 session 交互。
    pub agent_session_control: Option<Arc<dyn AgentSessionControl>>,
}

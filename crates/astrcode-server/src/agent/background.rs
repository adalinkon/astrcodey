//! 后台任务管理器。
//!
//! 管理被自动后台化的工具调用（主要是长时间运行的 shell 命令）。
//! 提供注册、查询、取消和清理能力。

use std::collections::HashMap;

use astrcode_core::{
    tool::ToolResult,
    types::{BackgroundTaskId, SessionId},
};

/// 后台任务的运行时摘要，供外部查询使用。
#[derive(Debug, Clone)]
pub struct TaskSummary {
    pub task_id: BackgroundTaskId,
    pub call_id: String,
    pub tool_name: String,
}

struct RunningTask {
    call_id: String,
    tool_name: String,
    session_id: SessionId,
    /// 后台执行任务的 tokio JoinHandle。
    handle: tokio::task::JoinHandle<()>,
}

/// 管理所有 session 的后台任务。
///
/// 当工具执行超过阈值时，agent loop 将其转入后台，把 `JoinHandle` 注册到这里。
/// 后台任务完成后通过 `actor_tx` 通知 handler 层。
pub struct BackgroundTaskManager {
    tasks: HashMap<BackgroundTaskId, RunningTask>,
}

impl BackgroundTaskManager {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }

    /// 注册一个后台任务。
    ///
    /// 调用方需要先把工具执行的 future spawn 为独立的 tokio task，
    /// 然后把 JoinHandle 连同元信息一起注册。
    pub fn register(
        &mut self,
        session_id: SessionId,
        call_id: String,
        tool_name: String,
        handle: tokio::task::JoinHandle<()>,
    ) -> BackgroundTaskId {
        let task_id = astrcode_core::types::new_background_task_id();
        self.tasks.insert(
            task_id.clone(),
            RunningTask {
                call_id,
                tool_name,
                session_id,
                handle,
            },
        );
        task_id
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

    /// 列出指定 session 的所有活跃后台任务摘要。
    pub fn list_for_session(&self, session_id: &SessionId) -> Vec<TaskSummary> {
        self.tasks
            .iter()
            .filter(|(_, task)| &task.session_id == session_id)
            .map(|(id, task)| TaskSummary {
                task_id: id.clone(),
                call_id: task.call_id.clone(),
                tool_name: task.tool_name.clone(),
            })
            .collect()
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
}

impl Default for BackgroundTaskManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 为后台化的工具调用构造占位 `ToolResult`。
///
/// LLM 收到这个结果后会知道任务已在后台运行，可以继续其他推理。
pub fn backgrounded_placeholder_result(
    _call_id: &str,
    task_id: &BackgroundTaskId,
    _tool_name: &str,
    command: Option<&str>,
) -> ToolResult {
    let mut content = format!(
        "Task moved to background (task: {task_id}). The result will be available in the next \
         turn."
    );
    if let Some(cmd) = command {
        content = format!("{content} Command: {cmd}");
    }

    let mut meta = std::collections::BTreeMap::new();
    meta.insert("backgrounded".into(), serde_json::json!(true));
    meta.insert("taskId".into(), serde_json::json!(task_id.to_string()));

    ToolResult {
        call_id: _call_id.to_string(),
        content,
        is_error: false,
        error: None,
        metadata: meta,
        duration_ms: None,
    }
}

/// 构造后台任务完成后的上下文注入文本，供下一轮 LLM 消息使用。
///
/// 当 handler 检测到 session 有已完成的后台任务时，在下一轮 submit_prompt
/// 开始前把结果以 system message 的形式注入到对话历史中。
#[allow(dead_code)]
pub fn completed_task_context_message(tool_name: &str, result: &ToolResult) -> String {
    let exit_info = if result.is_error {
        format!(
            "Failed: {}",
            result.error.as_deref().unwrap_or("unknown error")
        )
    } else {
        "Completed successfully.".to_string()
    };

    format!(
        "<completed-background-task>\nTool: \
         {tool_name}\n{exit_info}\nOutput:\n{}\n</completed-background-task>",
        result.content
    )
}

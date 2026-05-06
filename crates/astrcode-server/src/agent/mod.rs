//! Agent 模块 — 回合处理器与相关工具。

pub mod background;
pub(crate) mod compact;
mod r#loop;
pub(crate) mod post_compact;
pub(crate) mod tool_exec;
pub(crate) mod tool_types;
pub(crate) mod util;

pub use background::{BackgroundTaskManager, TaskSummary};
pub use compact::AutoCompactFailureTracker;
pub use r#loop::{AgentCompactContinuation, AgentError, AgentLoop, AgentServices, AgentTurnOutput};
pub(crate) use r#loop::{AgentSignal, drive_agent};

//! Tool 前端贡献的线缆契约（宿主 Web/TUI 按此选组件，不发给 LLM）。
//!
//! 扩展在 `Registrar::tool_ui` 注册；宿主在 `ToolCallCompleted.metadata.toolUi`
//! 及 SSE `patchMetadata` 中投影给前端。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// ToolResult / conversation block metadata 键。
pub const TOOL_UI_METADATA_KEY: &str = "toolUi";

/// 当前交互阶段（与 `tool_ui_phase` 常量配合）。
pub const TOOL_UI_PHASE_METADATA_KEY: &str = "toolUiPhase";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolUiWire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<ToolInputUiWire>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ToolApprovalUiWire>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ToolResultUiWire>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ToolInputUiWire {
    Schema {
        schema: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ui_schema: Option<Value>,
    },
    Builtin {
        variant: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ToolApprovalUiWire {
    /// 内置审批/交互卡片：`questionnaire` | `select` | `confirm` | `danger-confirm` | `diff-apply`
    Builtin { variant: String },
    Schema {
        schema: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ui_schema: Option<Value>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ToolResultUiWire {
    Builtin { variant: String },
}

//! s6r 协议类型 — WASM 扩展与宿主之间的消息契约。
//!
//! 协议版本 `S6R_VERSION = "1"`。guest 导出两个函数：
//!
//! - `extension_manifest() -> i64`：返回 ManifestMsg JSON
//! - `extension_call(req_ptr, req_len) -> i64`：返回 CallResponse JSON
//!
//! 两者均以 packed i64 `(ptr << 32 | len)` 传递 guest 内存地址。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::extension::{ExtensionEvent, HookMode};

/// s6r 协议当前版本。guest 的 `extension_manifest()` 中 `s6r` 字段必须等于此值。
pub const S6R_VERSION: &str = "1";

// ─── Manifest ────────────────────────────────────────────────────────────

/// `extension_manifest()` 返回的完整声明，包含扩展的全部静态元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// 协议版本，必须为 `S6R_VERSION`。
    pub s6r: String,
    /// 扩展唯一 ID。
    pub id: String,
    /// 扩展版本号（semver）。
    #[serde(default)]
    pub version: String,
    /// 可选描述。
    #[serde(default)]
    pub description: String,
    /// 申请的宿主能力（snake_case 字符串，对应 `ExtensionCapability`）。
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// 注册的工具列表。
    #[serde(default)]
    pub tools: Vec<ManifestTool>,
    /// 注册的斜杠命令列表。
    #[serde(default)]
    pub commands: Vec<ManifestCommand>,
    /// 订阅的 hook 列表。
    #[serde(default)]
    pub hooks: Vec<ManifestHook>,
}

/// manifest 中单个工具的声明。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestTool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    /// `"sequential"`（默认）或 `"parallel"`。
    #[serde(default = "sequential_mode")]
    pub mode: String,
}

fn sequential_mode() -> String {
    "sequential".into()
}

/// manifest 中单个命令的声明。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestCommand {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

/// manifest 中单个 hook 订阅的声明。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestHook {
    /// s6r 事件名，如 `"pre_tool_use"`。
    pub on: String,
    /// `"blocking"` | `"non_blocking"` | `"advisory"`。
    pub mode: String,
}

// ─── CallRequest ─────────────────────────────────────────────────────────

/// 宿主发给 `extension_call()` 的请求。
///
/// `call` 字段作为 serde tag，决定变体：
/// ```json
/// { "call": "tool",    "id": "req-1", "name": "grep", "input": {...} }
/// { "call": "hook",    "id": "req-2", "on": "pre_tool_use", "input": {...} }
/// { "call": "command", "id": "req-3", "name": "/hello", "input": {...} }
/// ```
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "call", rename_all = "snake_case")]
pub enum CallRequest {
    Tool {
        id: String,
        name: String,
        input: Value,
    },
    Hook {
        id: String,
        on: String,
        input: Value,
    },
    Command {
        id: String,
        name: String,
        input: Value,
    },
}

impl CallRequest {
    pub fn id(&self) -> &str {
        match self {
            Self::Tool { id, .. } => id,
            Self::Hook { id, .. } => id,
            Self::Command { id, .. } => id,
        }
    }
}

// ─── CallResponse ─────────────────────────────────────────────────────────

/// `extension_call()` 返回的响应。
///
/// 成功时 `ok = true`，`effect` 说明结果语义（默认 `"ok"`），
/// `data` 携带 effect 相关的附加数据。
/// 失败时 `ok = false`，`error` 携带描述。
///
/// ## effect 枚举
///
/// | effect | 适用场景 | data 字段 |
/// |--------|---------|-----------|
/// | `"ok"` | 默认成功 | 省略 |
/// | `"block"` | blocking hook 阻止操作 | `{ "reason": string }` |
/// | `"modified_input"` | pre_tool_use 修改入参 | `{ "tool_input": object }` |
/// | `"tool_outcome"` | 工具自定义执行结果 | `{ "outcome": object }` |
/// | `"prompt_contributions"` | PromptBuild 贡献 | `PromptContributions` |
/// | `"compact_contributions"` | Compact 贡献 | `CompactContributions` |
/// | `"replace_messages"` | Provider 替换消息 | `{ "messages": array }` |
/// | `"append_messages"` | Provider 追加消息 | `{ "messages": array }` |
#[derive(Debug, Serialize, Deserialize)]
pub struct CallResponse {
    /// 关联的请求 ID。
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl CallResponse {
    /// 返回 effect 字符串，默认为 `"ok"`。
    pub fn effect(&self) -> &str {
        self.effect.as_deref().unwrap_or("ok")
    }

    /// 从 `data[key]` 中获取字符串值。
    pub fn data_str(&self, key: &str) -> &str {
        self.data
            .as_ref()
            .and_then(|d| d[key].as_str())
            .unwrap_or("")
    }

    /// 从 `data[key]` 中获取 JSON 值。
    pub fn data_value(&self, key: &str) -> Option<&Value> {
        self.data.as_ref().and_then(|d| d.get(key))
    }
}

// ─── 事件名 / 模式名 ↔ Rust 类型转换 ─────────────────────────────────────

/// s6r 事件名字符串 → `ExtensionEvent`。未知名返回 `None`。
pub fn event_from_name(name: &str) -> Option<ExtensionEvent> {
    match name {
        "session_start" => Some(ExtensionEvent::SessionStart),
        "session_resume" => Some(ExtensionEvent::SessionResume),
        "session_shutdown" => Some(ExtensionEvent::SessionShutdown),
        "turn_start" => Some(ExtensionEvent::TurnStart),
        "turn_end" => Some(ExtensionEvent::TurnEnd),
        "turn_aborted" => Some(ExtensionEvent::TurnAborted),
        "step_start" => Some(ExtensionEvent::StepStart),
        "step_end" => Some(ExtensionEvent::StepEnd),
        "pre_tool_use" => Some(ExtensionEvent::PreToolUse),
        "post_tool_use" => Some(ExtensionEvent::PostToolUse),
        "post_tool_use_failure" => Some(ExtensionEvent::PostToolUseFailure),
        "before_provider_request" => Some(ExtensionEvent::BeforeProviderRequest),
        "after_provider_response" => Some(ExtensionEvent::AfterProviderResponse),
        "user_prompt_submit" => Some(ExtensionEvent::UserPromptSubmit),
        "prompt_build" => Some(ExtensionEvent::PromptBuild),
        "pre_compact" => Some(ExtensionEvent::PreCompact),
        "post_compact" => Some(ExtensionEvent::PostCompact),
        "post_recap" => Some(ExtensionEvent::PostRecap),
        _ => None,
    }
}

/// s6r 模式名字符串 → `HookMode`。未知名返回 `None`。
pub fn mode_from_name(name: &str) -> Option<HookMode> {
    match name {
        "blocking" => Some(HookMode::Blocking),
        "non_blocking" => Some(HookMode::NonBlocking),
        "advisory" => Some(HookMode::Advisory),
        _ => None,
    }
}

/// `ExtensionEvent` → s6r 事件名字符串。
pub fn event_to_name(event: &ExtensionEvent) -> &'static str {
    match event {
        ExtensionEvent::SessionStart => "session_start",
        ExtensionEvent::SessionResume => "session_resume",
        ExtensionEvent::SessionShutdown => "session_shutdown",
        ExtensionEvent::TurnStart => "turn_start",
        ExtensionEvent::TurnEnd => "turn_end",
        ExtensionEvent::TurnAborted => "turn_aborted",
        ExtensionEvent::StepStart => "step_start",
        ExtensionEvent::StepEnd => "step_end",
        ExtensionEvent::PreToolUse => "pre_tool_use",
        ExtensionEvent::PostToolUse => "post_tool_use",
        ExtensionEvent::PostToolUseFailure => "post_tool_use_failure",
        ExtensionEvent::BeforeProviderRequest => "before_provider_request",
        ExtensionEvent::AfterProviderResponse => "after_provider_response",
        ExtensionEvent::UserPromptSubmit => "user_prompt_submit",
        ExtensionEvent::PromptBuild => "prompt_build",
        ExtensionEvent::PreCompact => "pre_compact",
        ExtensionEvent::PostCompact => "post_compact",
        ExtensionEvent::PostRecap => "post_recap",
    }
}

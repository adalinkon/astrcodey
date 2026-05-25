//! WASM ABI 协议常量 — host 和 guest 都要遵守的判别值和 effect code。
//!
//! 这些常量定义了 WASM 插件和宿主之间的通信协议。
//! 放在 SDK 里是因为 host 侧和 guest 侧都需要遵守同一套判别值。

use crate::extension::{ExtensionEvent, HookMode};
use crate::tool::ExecutionMode;

// ─── Discriminant helpers ───────────────────────────────────────────────

pub const fn event_discriminant(event: ExtensionEvent) -> u8 {
    match event {
        ExtensionEvent::SessionStart => 0,
        ExtensionEvent::SessionShutdown => 1,
        ExtensionEvent::TurnStart => 2,
        ExtensionEvent::TurnEnd => 3,
        ExtensionEvent::PreToolUse => 4,
        ExtensionEvent::PostToolUse => 5,
        ExtensionEvent::BeforeProviderRequest => 6,
        ExtensionEvent::AfterProviderResponse => 7,
        ExtensionEvent::UserPromptSubmit => 8,
        ExtensionEvent::PromptBuild => 9,
        ExtensionEvent::PreCompact => 10,
        ExtensionEvent::PostCompact => 11,
        ExtensionEvent::TurnAborted => 12,
        ExtensionEvent::PostToolUseFailure => 13,
        ExtensionEvent::StepStart => 14,
        ExtensionEvent::StepEnd => 15,
        ExtensionEvent::PostRecap => 16,
        ExtensionEvent::SessionResume => 17,
    }
}

pub fn event_from_discriminant(d: u8) -> Option<ExtensionEvent> {
    match d {
        0 => Some(ExtensionEvent::SessionStart),
        1 => Some(ExtensionEvent::SessionShutdown),
        2 => Some(ExtensionEvent::TurnStart),
        3 => Some(ExtensionEvent::TurnEnd),
        4 => Some(ExtensionEvent::PreToolUse),
        5 => Some(ExtensionEvent::PostToolUse),
        6 => Some(ExtensionEvent::BeforeProviderRequest),
        7 => Some(ExtensionEvent::AfterProviderResponse),
        8 => Some(ExtensionEvent::UserPromptSubmit),
        9 => Some(ExtensionEvent::PromptBuild),
        10 => Some(ExtensionEvent::PreCompact),
        11 => Some(ExtensionEvent::PostCompact),
        12 => Some(ExtensionEvent::TurnAborted),
        13 => Some(ExtensionEvent::PostToolUseFailure),
        14 => Some(ExtensionEvent::StepStart),
        15 => Some(ExtensionEvent::StepEnd),
        16 => Some(ExtensionEvent::PostRecap),
        17 => Some(ExtensionEvent::SessionResume),
        _ => None,
    }
}

pub const fn mode_discriminant(mode: HookMode) -> u8 {
    match mode {
        HookMode::Blocking => 0,
        HookMode::NonBlocking => 1,
        HookMode::Advisory => 2,
    }
}

pub fn mode_from_discriminant(d: u8) -> Option<HookMode> {
    match d {
        0 => Some(HookMode::Blocking),
        1 => Some(HookMode::NonBlocking),
        2 => Some(HookMode::Advisory),
        _ => None,
    }
}

// ─── Tool execution mode discriminants ───────────────────────────────────

pub const fn execution_mode_discriminant(mode: ExecutionMode) -> u8 {
    match mode {
        ExecutionMode::Sequential => 0,
        ExecutionMode::Parallel => 1,
    }
}

pub fn execution_mode_from_discriminant(d: u8) -> ExecutionMode {
    match d {
        1 => ExecutionMode::Parallel,
        _ => ExecutionMode::Sequential,
    }
}

// ─── Guest response effect codes ─────────────────────────────────────────

/// WASM guest `handle_event` / `handle_tool` 返回的 effect code。
pub const GUEST_EFFECT_OK: i8 = 0;
/// 操作失败，content 为错误信息。
pub const GUEST_EFFECT_ERROR: i8 = 1;
/// 工具执行结果包含 `RunSession` outcome。
pub const GUEST_EFFECT_TOOL_OUTCOME: i8 = 2;
/// `PreToolUse` 返回 `ModifiedInput`，content 为新 tool_input JSON。
pub const GUEST_EFFECT_MODIFIED_INPUT: i8 = 3;
/// `PromptBuild` 返回贡献，content 为 `PromptContributions` JSON。
pub const GUEST_EFFECT_PROMPT_CONTRIBUTIONS: i8 = 4;
/// `Compact` 返回贡献，content 为 `CompactContributions` JSON。
pub const GUEST_EFFECT_COMPACT_CONTRIBUTIONS: i8 = 5;
/// `Provider` 返回 `ReplaceMessages`，content 为 messages JSON。
pub const GUEST_EFFECT_REPLACE_MESSAGES: i8 = 6;
/// `Provider` 返回 `AppendMessages`，content 为 messages JSON。
pub const GUEST_EFFECT_APPEND_MESSAGES: i8 = 7;

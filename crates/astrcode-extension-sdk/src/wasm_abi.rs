//! WASM ABI 协议绑定 — 重导出 s6r 协议类型，供宿主和 guest 双侧共享。
//!
//! 旧版的魔术整数判别符（`GUEST_EFFECT_OK = 0` 等）和事件判别符已由 s6r
//! 协议的字符串 effect 取代，不再需要。

pub use crate::s6r::{
    CallRequest, CallResponse, Manifest, ManifestCommand, ManifestHook, ManifestTool,
    S6R_VERSION, event_from_name, event_to_name, mode_from_name,
};

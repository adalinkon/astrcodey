//! WASM `host_emit` 宿主后端：权限校验、声明校验与事件投递。
//!
//! 与 [`crate::runner`] 中 `BoundExtensionEventSink` 共用同一套校验规则。

use std::collections::HashMap;

use astrcode_core::{
    event::EventPayload,
    extension::{ExtensionCapability, ExtensionError, ExtensionEventDecl},
};
use tokio::sync::mpsc;

/// `host_emit` 输入 JSON（EmitEventMsg）。
#[derive(serde::Deserialize)]
struct EmitEventMsg {
    event_type: String,
    #[serde(default = "default_schema_version")]
    schema_version: u32,
    #[serde(default)]
    payload: serde_json::Value,
}

fn default_schema_version() -> u32 {
    1
}

/// manifest 是否声明了 `emit_events`。
pub fn authorize_emit(declared: &[ExtensionCapability]) -> Result<(), String> {
    if declared.contains(&ExtensionCapability::EmitEvents) {
        Ok(())
    } else {
        Err("permission denied: emit_events not declared".into())
    }
}

/// 解析 guest 传入的 EmitEventMsg JSON。
pub fn parse_emit_request(json: &str) -> Result<(String, u32, serde_json::Value), String> {
    let msg: EmitEventMsg =
        serde_json::from_str(json).map_err(|e| format!("invalid EmitEventMsg JSON: {e}"))?;
    if msg.event_type.trim().is_empty() {
        return Err("event_type is empty".into());
    }
    Ok((msg.event_type, msg.schema_version, msg.payload))
}

/// 校验声明并将事件写入会话事件通道（同步，不阻塞 async runtime worker）。
pub fn try_emit_to_channel(
    extension_id: &str,
    declarations: &HashMap<String, ExtensionEventDecl>,
    event_tx: &mpsc::UnboundedSender<EventPayload>,
    event_type: &str,
    schema_version: u32,
    payload: serde_json::Value,
) -> Result<(), String> {
    validate_emit(declarations, event_type, schema_version, &payload)?;
    event_tx
        .send(EventPayload::ExtensionEvent {
            extension_id: extension_id.to_owned(),
            event_type: event_type.to_owned(),
            schema_version,
            payload,
        })
        .map_err(|_| "event channel closed".into())
}

fn validate_emit(
    declarations: &HashMap<String, ExtensionEventDecl>,
    event_type: &str,
    schema_version: u32,
    payload: &serde_json::Value,
) -> Result<(), String> {
    let decl = declarations
        .get(event_type)
        .ok_or_else(|| format!("undeclared extension event type: {event_type}"))?;

    if schema_version > decl.schema_version {
        return Err(format!(
            "schema_version {schema_version} exceeds declared {} for {event_type}",
            decl.schema_version
        ));
    }

    let serialized =
        serde_json::to_string(payload).map_err(|e| format!("serialize payload: {e}"))?;
    if serialized.len() > decl.max_payload_bytes {
        return Err(format!(
            "payload exceeds {} bytes for {event_type}",
            decl.max_payload_bytes
        ));
    }
    Ok(())
}

/// 将 [`ExtensionEventDecl`] 切片转为查找表。
pub fn decls_to_map(decls: &[ExtensionEventDecl]) -> HashMap<String, ExtensionEventDecl> {
    decls
        .iter()
        .map(|d| (d.event_type.clone(), d.clone()))
        .collect()
}

/// 供 runner 内 `ExtensionEventSink` 实现复用。
pub fn emit_for_sink(
    extension_id: &str,
    declarations: &HashMap<String, ExtensionEventDecl>,
    event_tx: &mpsc::UnboundedSender<EventPayload>,
    event_type: &str,
    schema_version: u32,
    payload: serde_json::Value,
) -> Result<(), ExtensionError> {
    try_emit_to_channel(
        extension_id,
        declarations,
        event_tx,
        event_type,
        schema_version,
        payload,
    )
    .map_err(ExtensionError::Internal)
}

/// 成功响应 JSON（写入 guest 的 ResultMsg）。
pub fn ok_result() -> String {
    serde_json::json!({ "ok": true }).to_string()
}

/// 失败响应 JSON。
pub fn err_result(error: impl std::fmt::Display) -> String {
    serde_json::json!({ "ok": false, "error": error.to_string() }).to_string()
}

#[cfg(test)]
mod tests {
    use astrcode_core::extension::ExtensionEventDecl;

    use super::*;

    fn sample_decl() -> HashMap<String, ExtensionEventDecl> {
        decls_to_map(&[ExtensionEventDecl {
            event_type: "demo.event".into(),
            schema_version: 1,
            durable: true,
            max_payload_bytes: 1024,
        }])
    }

    #[test]
    fn authorize_emit_requires_capability() {
        assert!(authorize_emit(&[]).is_err());
        authorize_emit(&[ExtensionCapability::EmitEvents]).unwrap();
    }

    #[test]
    fn try_emit_rejects_undeclared_type() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let err = try_emit_to_channel(
            "ext-1",
            &sample_decl(),
            &tx,
            "other",
            1,
            serde_json::json!({}),
        )
        .unwrap_err();
        assert!(err.contains("undeclared"));
    }

    #[test]
    fn try_emit_accepts_declared_event() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        try_emit_to_channel(
            "ext-1",
            &sample_decl(),
            &tx,
            "demo.event",
            1,
            serde_json::json!({"k": 1}),
        )
        .unwrap();
        let payload = rx.try_recv().unwrap();
        assert!(matches!(
            payload,
            EventPayload::ExtensionEvent {
                extension_id,
                event_type,
                ..
            } if extension_id == "ext-1" && event_type == "demo.event"
        ));
    }
}

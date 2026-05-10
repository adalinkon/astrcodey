//! 无头执行模式 —— 单次提示执行（进程内）。
//!
//! 该模块实现了 CLI 的 `exec` 子命令，用于在不需要交互式 TUI 的情况下
//! 一次性提交提示并输出结果。支持纯文本和 JSONL 两种输出格式。

use std::io::Write;

use astrcode_client::client::AstrcodeClient;
use astrcode_core::event::EventPayload;
use astrcode_protocol::{commands::ClientCommand, events::ClientNotification};

use crate::transport::InProcessTransport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotificationAction {
    Continue,
    Finish,
}

/// 执行单次提示并等待响应完成。
pub async fn run(prompt: &str, jsonl: bool, timeout_secs: u64) -> Result<(), String> {
    let client = AstrcodeClient::new(InProcessTransport::start());

    let _sid = client
        .create_session(".")
        .await
        .map_err(|e| format!("Cannot create session: {e}"))?;

    let mut stream = client
        .subscribe_events()
        .await
        .map_err(|e| format!("Cannot subscribe: {e}"))?;

    client
        .send_command(&ClientCommand::SubmitPrompt {
            text: prompt.into(),
            attachments: vec![],
        })
        .await
        .map_err(|e| format!("Cannot submit: {e}"))?;

    let deadline = (timeout_secs > 0)
        .then(|| tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout_secs));

    loop {
        let recv_result = if let Some(deadline) = deadline {
            tokio::time::timeout_at(deadline, stream.recv())
                .await
                .map_err(|_| format!("exec timed out after {timeout_secs}s"))?
        } else {
            stream.recv().await
        };
        let notification = match recv_result {
            Ok(n) => n,
            Err(_) => break,
        };
        let action = render_notification(
            &notification,
            jsonl,
            &mut std::io::stdout(),
            &mut std::io::stderr(),
        )?;
        if action == NotificationAction::Finish {
            break;
        }
    }
    Ok(())
}

fn render_notification(
    notification: &ClientNotification,
    jsonl: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> Result<NotificationAction, String> {
    if jsonl {
        write_jsonl(notification, out)?;
        return Ok(notification_action(notification));
    }

    match notification {
        ClientNotification::Event(core_event) => match &core_event.payload {
            EventPayload::AssistantTextDelta { delta, .. } => {
                write!(out, "{delta}").map_err(|e| format!("write stdout: {e}"))?;
                Ok(NotificationAction::Continue)
            },
            EventPayload::TurnCompleted { .. } => {
                writeln!(out).map_err(|e| format!("write stdout: {e}"))?;
                Ok(NotificationAction::Finish)
            },
            EventPayload::ErrorOccurred { message, .. } => {
                writeln!(err, "Error: {message}").map_err(|e| format!("write stderr: {e}"))?;
                Ok(NotificationAction::Finish)
            },
            _ => Ok(NotificationAction::Continue),
        },
        ClientNotification::Error { message, .. } => {
            writeln!(err, "Error: {message}").map_err(|e| format!("write stderr: {e}"))?;
            Ok(NotificationAction::Finish)
        },
        _ => Ok(NotificationAction::Continue),
    }
}

fn write_jsonl(notification: &ClientNotification, out: &mut impl Write) -> Result<(), String> {
    serde_json::to_writer(&mut *out, notification).map_err(|e| format!("serialize jsonl: {e}"))?;
    writeln!(out).map_err(|e| format!("write stdout: {e}"))
}

fn notification_action(notification: &ClientNotification) -> NotificationAction {
    match notification {
        ClientNotification::Event(core_event) => match core_event.payload {
            EventPayload::TurnCompleted { .. } | EventPayload::ErrorOccurred { .. } => {
                NotificationAction::Finish
            },
            _ => NotificationAction::Continue,
        },
        ClientNotification::Error { .. } => NotificationAction::Finish,
        _ => NotificationAction::Continue,
    }
}

#[cfg(test)]
mod tests {
    use astrcode_core::{
        event::{Event, EventPayload},
        types::SessionId,
    };

    use super::*;

    fn notification(payload: EventPayload) -> ClientNotification {
        ClientNotification::Event(Event::new(SessionId::from("session-1"), None, payload))
    }

    #[test]
    fn jsonl_output_includes_streaming_delta() {
        let notification = notification(EventPayload::AssistantTextDelta {
            message_id: "message-1".into(),
            delta: "hello".into(),
        });
        let mut out = Vec::new();
        let mut err = Vec::new();

        let action = render_notification(&notification, true, &mut out, &mut err).unwrap();

        let line: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(line["event"], "event");
        assert_eq!(line["data"]["type"], "assistant_text_delta");
        assert_eq!(line["data"]["delta"], "hello");
        assert!(err.is_empty());
        assert_eq!(action, NotificationAction::Continue);
    }

    #[test]
    fn jsonl_output_includes_turn_completion_before_finishing() {
        let notification = notification(EventPayload::TurnCompleted {
            finish_reason: "stop".into(),
        });
        let mut out = Vec::new();
        let mut err = Vec::new();

        let action = render_notification(&notification, true, &mut out, &mut err).unwrap();

        let line: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(line["data"]["type"], "turn_completed");
        assert!(err.is_empty());
        assert_eq!(action, NotificationAction::Finish);
    }

    #[test]
    fn text_output_keeps_plain_transcript_behavior() {
        let notification = notification(EventPayload::AssistantTextDelta {
            message_id: "message-1".into(),
            delta: "hello".into(),
        });
        let mut out = Vec::new();
        let mut err = Vec::new();

        let action = render_notification(&notification, false, &mut out, &mut err).unwrap();

        assert_eq!(String::from_utf8(out).unwrap(), "hello");
        assert!(err.is_empty());
        assert_eq!(action, NotificationAction::Continue);
    }
}

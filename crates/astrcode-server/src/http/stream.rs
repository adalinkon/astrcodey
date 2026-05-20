//! `GET /api/sessions/{id}/stream` SSE 流。
//!
//! 三段流串联：
//! 1. `replay_error_stream`：cursor 解析或重放失败时推一条 `RehydrateRequired`。
//! 2. `replay_stream`：从 `EventStore` 拉历史事件转 deltas（按 cursor 起点）。
//! 3. `live_stream`：订阅 `ServerEventBus` 的 broadcast，过滤 sid，推增量。 Lagged 时自发一条
//!    `RehydrateRequired` 让客户端重新拉快照。

use std::{collections::HashMap, sync::Arc};

use astrcode_core::{
    event::Event,
    types::{Cursor, SessionId},
};
use astrcode_protocol::{
    events::ClientNotification,
    http::{
        ConversationBlockDto, ConversationCursorDto, ConversationDeltaDto,
        ConversationStreamEnvelopeDto,
    },
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event as SseEvent, KeepAlive, Sse},
    },
};
use futures_util::{StreamExt, stream};
use serde::Deserialize;
use tokio::sync::broadcast;

use super::{
    HttpState, error_response,
    projection::{live::event_to_deltas, replay::event_to_replay_deltas},
};
use crate::bootstrap::ServerRuntime;

#[derive(Debug, Deserialize)]
pub(super) struct StreamQuery {
    cursor: Option<String>,
}

pub(in crate::http) async fn session_stream(
    State(http_state): State<HttpState>,
    Path(raw_session_id): Path<String>,
    Query(query): Query<StreamQuery>,
) -> Response {
    tracing::info!(session_id = %raw_session_id, cursor = ?query.cursor, "SSE stream connected");
    let session_id = SessionId::from(raw_session_id);

    // Validate session exists before opening the stream.
    if http_state
        .runtime
        .session_manager
        .read_model(&session_id)
        .await
        .is_err()
    {
        return error_response(
            StatusCode::NOT_FOUND,
            "session_not_found",
            "Session not found",
        );
    }

    let rx = http_state.event_bus.broadcast_sender().subscribe();
    let (missed_events, replay_error) = match query.cursor.as_ref() {
        Some(cursor) if cursor.parse::<u64>().is_err() => (Vec::new(), true),
        Some(cursor) => match http_state
            .runtime
            .session_manager
            .replay_from(&session_id, &Cursor::from(cursor.as_str()))
            .await
        {
            Ok(events) => (events, false),
            Err(error) => {
                tracing::warn!(session_id = %session_id, cursor, "failed to replay SSE cursor: {error}");
                (Vec::new(), true)
            },
        },
        None => (Vec::new(), false),
    };
    let replay_max_seq = missed_events.iter().filter_map(|event| event.seq).max();
    let replay_runtime = Arc::clone(&http_state.runtime);
    let replay_session_id = session_id.clone();
    let replay_stream = stream::iter(missed_events)
        .then(move |event| {
            let runtime = Arc::clone(&replay_runtime);
            let replay_sid = replay_session_id.clone();
            async move {
                let deltas = event_to_replay_deltas(&event);
                let cursor = event_cursor(&runtime, &event).await;
                deltas
                    .into_iter()
                    .map(|delta| {
                        Ok(sse_event(&ConversationStreamEnvelopeDto {
                            session_id: replay_sid.to_string(),
                            cursor: ConversationCursorDto {
                                value: cursor.clone(),
                            },
                            delta,
                        }))
                    })
                    .collect::<Vec<_>>()
            }
        })
        .flat_map(stream::iter);
    let replay_error_stream = stream::iter(replay_error.then(|| {
        Ok(sse_event(&ConversationStreamEnvelopeDto {
            session_id: session_id.to_string(),
            cursor: ConversationCursorDto { value: "0".into() },
            delta: ConversationDeltaDto::RehydrateRequired,
        }))
    }));

    let live_runtime = Arc::clone(&http_state.runtime);
    let live_stream = stream::unfold(
        (
            rx,
            live_runtime,
            session_id,
            replay_max_seq,
            false,
            std::collections::VecDeque::<
                Result<axum::response::sse::Event, std::convert::Infallible>,
            >::new(),
            HashMap::<String, String>::new(),
        ),
        |(mut rx, runtime, session_id, replay_max_seq, closing, mut pending, mut tool_args)| async move {
            if closing {
                return None;
            }

            if let Some(item) = pending.pop_front() {
                return Some((
                    item,
                    (
                        rx,
                        runtime,
                        session_id,
                        replay_max_seq,
                        false,
                        pending,
                        tool_args,
                    ),
                ));
            }

            loop {
                match rx.recv().await {
                    Ok(ClientNotification::Event(event)) if event.session_id == session_id => {
                        if replay_max_seq
                            .zip(event.seq)
                            .is_some_and(|(max_seq, event_seq)| event_seq <= max_seq)
                        {
                            continue;
                        }
                        let mut deltas = event_to_deltas(&event);
                        if deltas.is_empty() {
                            continue;
                        }
                        // Track arguments from PatchArguments deltas.
                        for delta in &deltas {
                            if let ConversationDeltaDto::PatchArguments {
                                block_id,
                                arguments,
                            } = delta
                            {
                                tool_args.insert(block_id.clone(), arguments.clone());
                            }
                        }
                        // Fill in arguments for FinalizeBlock tool calls.
                        for delta in &mut deltas {
                            if let ConversationDeltaDto::FinalizeBlock {
                                block: ConversationBlockDto::ToolCall { id, arguments, .. },
                            } = delta
                            {
                                if arguments.is_empty() {
                                    if let Some(args) = tool_args.remove(id) {
                                        *arguments = args;
                                    }
                                }
                            }
                        }
                        let cursor = event_cursor(&runtime, &event).await;
                        let items: std::collections::VecDeque<_> = deltas
                            .into_iter()
                            .map(|delta| {
                                Ok(sse_event(&ConversationStreamEnvelopeDto {
                                    session_id: session_id.to_string(),
                                    cursor: ConversationCursorDto {
                                        value: cursor.clone(),
                                    },
                                    delta,
                                }))
                            })
                            .collect();
                        let mut items = items;
                        let Some(first) = items.pop_front() else {
                            continue;
                        };
                        return Some((
                            first,
                            (
                                rx,
                                runtime,
                                session_id,
                                replay_max_seq,
                                false,
                                items,
                                tool_args,
                            ),
                        ));
                    },
                    Ok(_) => {},
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let cursor = state_cursor(&runtime, &session_id).await;
                        let item = Ok(sse_event(&ConversationStreamEnvelopeDto {
                            session_id: session_id.to_string(),
                            cursor: ConversationCursorDto { value: cursor },
                            delta: ConversationDeltaDto::RehydrateRequired,
                        }));
                        return Some((
                            item,
                            (
                                rx,
                                runtime,
                                session_id,
                                replay_max_seq,
                                true,
                                pending,
                                tool_args,
                            ),
                        ));
                    },
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        },
    );
    let stream = replay_error_stream.chain(replay_stream).chain(live_stream);
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn event_cursor(runtime: &ServerRuntime, event: &Event) -> String {
    if let Some(seq) = event.seq {
        seq.to_string()
    } else {
        state_cursor(runtime, &event.session_id).await
    }
}

async fn state_cursor(runtime: &ServerRuntime, session_id: &SessionId) -> String {
    match runtime.session_manager.latest_cursor(session_id).await {
        Ok(Some(cursor)) => cursor,
        Ok(None) => "0".to_string(),
        Err(error) => {
            tracing::warn!(
                session_id = %session_id,
                %error,
                "failed to read latest cursor from storage, falling back to 0"
            );
            "0".to_string()
        },
    }
}

fn sse_event<T: serde::Serialize>(value: &T) -> SseEvent {
    let data = serde_json::to_string(value).unwrap_or_else(|_| "{}".into());
    SseEvent::default().event("conversation").data(data)
}

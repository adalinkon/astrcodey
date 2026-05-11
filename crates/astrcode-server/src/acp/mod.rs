//! ACP (Agent Client Protocol) server adapter.
//!
//! Bridges the ACP JSON-RPC protocol (over stdio) to astrcode's internal
//! CommandHandle / broadcast event architecture. This module is purely a
//! DTO-mapping boundary — no session-runtime types leak through.

mod events;
mod handler;

use std::sync::Arc;

use agent_client_protocol::{
    Agent, ByteStreams, Client, ConnectionTo, Dispatch, Responder,
    schema::{
        AgentCapabilities, AgentNotification, CancelNotification, InitializeRequest,
        InitializeResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
        ProtocolVersion, SessionId as AcpSessionId, StopReason,
    },
};
use astrcode_core::types::SessionId;
use astrcode_protocol::events::ClientNotification;
use tokio::sync::broadcast;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use self::handler::{ActiveTurnTracker, TurnOutcome};
use crate::{bootstrap::ServerRuntime, handler::CommandHandle};

/// Run the ACP server, reading from stdin and writing to stdout.
///
/// This function blocks until the connection is closed or an unrecoverable
/// error occurs.
pub async fn run_acp_server(runtime: Arc<ServerRuntime>) -> agent_client_protocol::Result<()> {
    let (event_tx, _) = broadcast::channel(256);
    let command_handle = CommandHandle::spawn(runtime, event_tx.clone());
    let turn_tracker = Arc::new(ActiveTurnTracker::new());

    Agent
        .builder()
        .name("astrcode")
        .on_receive_request(
            {
                async move |req: InitializeRequest,
                            responder: Responder<InitializeResponse>,
                            _cx: ConnectionTo<Client>| {
                    let _ = req; // accept whatever version the client sends
                    responder.respond(
                        InitializeResponse::new(ProtocolVersion::V1)
                            .agent_capabilities(AgentCapabilities::new())
                            .agent_info(agent_client_protocol::schema::Implementation::new(
                                "astrcode",
                                env!("CARGO_PKG_VERSION"),
                            )),
                    )
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let command_handle = command_handle.clone();

                async move |req: NewSessionRequest,
                            responder: Responder<NewSessionResponse>,
                            _cx: ConnectionTo<Client>| {
                    let working_dir = req.cwd.to_string_lossy().to_string();
                    match command_handle.create_session(working_dir).await {
                        Ok(session_id) => {
                            let acp_sid = AcpSessionId::new(session_id.to_string());
                            responder.respond(NewSessionResponse::new(acp_sid))
                        },
                        Err(e) => responder.respond_with_internal_error(e.to_string()),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let command_handle = command_handle.clone();
                let turn_tracker = Arc::clone(&turn_tracker);
                let event_tx = event_tx.clone();

                async move |req: PromptRequest,
                            responder: Responder<PromptResponse>,
                            cx: ConnectionTo<Client>| {
                    let stop_reason =
                        handle_prompt(req, &command_handle, &turn_tracker, &event_tx, &cx).await;
                    responder.respond(PromptResponse::new(stop_reason))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let command_handle = command_handle.clone();
                let turn_tracker = Arc::clone(&turn_tracker);

                async move |notif: CancelNotification, _cx: ConnectionTo<Client>| {
                    let sid = SessionId::from(notif.session_id.to_string());
                    // Cancel pending turn waiters so the prompt handler returns Cancelled.
                    turn_tracker.cancel_session(&sid);
                    let _ = command_handle.abort_session(sid).await;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_dispatch(
            async move |message: Dispatch, cx: ConnectionTo<Client>| {
                message.respond_with_error(
                    agent_client_protocol::schema::Error::method_not_found(),
                    cx,
                )
            },
            agent_client_protocol::on_receive_dispatch!(),
        )
        .connect_to(ByteStreams::new(
            tokio::io::stdout().compat_write(),
            tokio::io::stdin().compat(),
        ))
        .await
}

async fn handle_prompt(
    req: PromptRequest,
    command_handle: &CommandHandle,
    turn_tracker: &ActiveTurnTracker,
    event_tx: &broadcast::Sender<ClientNotification>,
    cx: &ConnectionTo<Client>,
) -> StopReason {
    let session_id = SessionId::from(req.session_id.to_string());

    // Extract text from prompt content blocks.
    let text = extract_text(&req.prompt);

    let turn_id = match command_handle
        .submit_prompt_for_session(session_id.clone(), text)
        .await
    {
        Ok(id) => id,
        Err(e) => {
            tracing::error!(error = %e, "submit_prompt_for_session failed");
            return StopReason::EndTurn;
        },
    };

    // Register this turn so we can wait for completion.
    let mut completion_rx = turn_tracker.register_turn(session_id.clone(), turn_id.clone());

    // Subscribe to broadcast events for this session.
    let mut event_rx = event_tx.subscribe();

    loop {
        tokio::select! {
            biased;

            result = &mut completion_rx => {
                let stop_reason = match result {
                    Ok(TurnOutcome::Completed { stop_reason }) => stop_reason,
                    Err(_) => {
                        // Sender dropped = cancelled.
                        StopReason::Cancelled
                    }
                };
                // Drain any remaining events before responding.
                drain_events(&mut event_rx, &session_id, cx).await;
                return stop_reason;
            }

            notification = event_rx.recv() => {
                match notification {
                    Ok(ClientNotification::Event(event)) => {
                        if event.session_id != session_id {
                            continue;
                        }

                        // Check for TurnCompleted — this is our signal.
                        if let astrcode_core::event::EventPayload::TurnCompleted { finish_reason } = &event.payload {
                            let stop_reason = match finish_reason.as_str() {
                                "aborted" | "cancelled" => StopReason::Cancelled,
                                _ => StopReason::EndTurn,
                            };
                            turn_tracker.resolve_turn(
                                &session_id,
                                &turn_id,
                                TurnOutcome::Completed { stop_reason },
                            );
                            continue;
                        }

                        // Forward matching events as ACP notifications.
                        if let Some(acp_notif) = events::to_session_notification(
                            event.session_id.as_str(),
                            &event.payload,
                        ) {
                            let agent_notif = AgentNotification::SessionNotification(acp_notif);
                            let _ = cx.send_notification(agent_notif);
                        }

                        // Check for error events that should end the turn.
                        if let astrcode_core::event::EventPayload::ErrorOccurred { recoverable: false, .. } = &event.payload {
                            turn_tracker.resolve_turn(
                                &session_id,
                                &turn_id,
                                TurnOutcome::Completed { stop_reason: StopReason::EndTurn },
                            );
                            continue;
                        }
                    }
                    Ok(_) => {
                        // Non-Event notifications (SessionResumed, etc.) — ignore for ACP.
                    }
                    Err(broadcast::error::RecvError::Lagged(count)) => {
                        tracing::warn!(count, "ACP event subscriber lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // Broadcast channel closed — shut down.
                        turn_tracker.resolve_turn(
                            &session_id,
                            &turn_id,
                            TurnOutcome::Completed { stop_reason: StopReason::EndTurn },
                        );
                    }
                }
            }
        }
    }
}

/// Drain remaining events from the broadcast channel for this session
/// and forward them as ACP notifications before the prompt response.
async fn drain_events(
    event_rx: &mut broadcast::Receiver<ClientNotification>,
    session_id: &SessionId,
    cx: &ConnectionTo<Client>,
) {
    // Give a short window for in-flight events to arrive.
    let _ = tokio::time::timeout(std::time::Duration::from_millis(100), async {
        loop {
            match event_rx.try_recv() {
                Ok(ClientNotification::Event(event)) => {
                    if event.session_id == *session_id {
                        if let Some(acp_notif) = events::to_session_notification(
                            event.session_id.as_str(),
                            &event.payload,
                        ) {
                            let agent_notif = AgentNotification::SessionNotification(acp_notif);
                            let _ = cx.send_notification(agent_notif);
                        }
                    }
                },
                Ok(_) => {},
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(_) => break,
            }
        }
    })
    .await;
}

/// Extract plain text from ACP content blocks.
fn extract_text(blocks: &[agent_client_protocol::schema::ContentBlock]) -> String {
    let mut text = String::new();
    for block in blocks {
        if let agent_client_protocol::schema::ContentBlock::Text(tc) = block {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&tc.text);
        }
    }
    text
}

//! Per-session state management for ACP.

use agent_client_protocol::schema::StopReason;
use astrcode_core::types::SessionId;
use dashmap::DashMap;
use tokio::sync::oneshot;

/// Tracks active turns across sessions. Keyed by `(session_id, turn_id)`.
pub(crate) struct ActiveTurnTracker {
    /// Maps (session_id, turn_id) → oneshot sender for turn completion.
    turns: DashMap<(String, String), oneshot::Sender<TurnOutcome>>,
}

pub(crate) enum TurnOutcome {
    Completed { stop_reason: StopReason },
}

impl ActiveTurnTracker {
    pub fn new() -> Self {
        Self {
            turns: DashMap::new(),
        }
    }

    /// Register a pending turn. Returns the receiver that resolves when the turn finishes.
    pub fn register_turn(
        &self,
        session_id: SessionId,
        turn_id: astrcode_core::types::TurnId,
    ) -> oneshot::Receiver<TurnOutcome> {
        let (tx, rx) = oneshot::channel();
        self.turns
            .insert((session_id.into_string(), turn_id.into_string()), tx);
        rx
    }

    /// Resolve a turn with the given outcome. Drops the oneshot sender if the
    /// receiver was already dropped (e.g. prompt handler cancelled).
    pub fn resolve_turn(
        &self,
        session_id: &SessionId,
        turn_id: &astrcode_core::types::TurnId,
        outcome: TurnOutcome,
    ) {
        if let Some((_, tx)) = self
            .turns
            .remove(&(session_id.to_string(), turn_id.to_string()))
        {
            let _ = tx.send(outcome);
        }
    }

    /// Cancel all pending turns for a session.
    pub fn cancel_session(&self, session_id: &SessionId) {
        let prefix = session_id.to_string();
        self.turns.retain(|(sid, _), _| {
            if sid == &prefix {
                // Removing the entry drops the sender, which will cause the
                // receiver to get a RecvError — the prompt handler interprets
                // that as cancellation.
                false
            } else {
                true
            }
        });
    }
}

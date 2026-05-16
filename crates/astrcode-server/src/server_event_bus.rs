//! ServerEventBus — 持久化到 EventStore + 广播到客户端。
//!
//! 唯一的事件发射路径。Actor、Turn task、Background task 共用此类型，
//! 避免持久化/广播逻辑散落在多处。

use std::sync::Arc;

use astrcode_core::{
    event::Event,
    storage::EventStore,
    types::{SessionId, TurnId},
};
use astrcode_protocol::events::ClientNotification;
use astrcode_session::EventBus;
use tokio::sync::broadcast;

pub struct ServerEventBus {
    store: Arc<dyn EventStore>,
    tx: broadcast::Sender<ClientNotification>,
}

impl ServerEventBus {
    pub fn new(store: Arc<dyn EventStore>, tx: broadcast::Sender<ClientNotification>) -> Self {
        Self { store, tx }
    }

    /// 返回内部 broadcast sender 的引用。
    pub fn broadcast_sender(&self) -> &broadcast::Sender<ClientNotification> {
        &self.tx
    }

    /// 广播任意 ClientNotification（如 SessionResumed、Error 等）。
    pub fn send_notification(&self, notification: ClientNotification) {
        let _ = self.tx.send(notification);
    }
}

#[async_trait::async_trait]
impl EventBus for ServerEventBus {
    async fn emit(
        &self,
        session_id: &SessionId,
        turn_id: Option<&TurnId>,
        payload: astrcode_core::event::EventPayload,
    ) {
        let event = Event::new(session_id.clone(), turn_id.cloned(), payload);
        if event.payload.is_durable() {
            if let Err(e) = self.store.append_event(event.clone()).await {
                tracing::error!(session_id = %session_id, error = %e, "failed to persist event via EventBus");
                return;
            }
        }
        let _ = self.tx.send(ClientNotification::Event(event));
    }
}

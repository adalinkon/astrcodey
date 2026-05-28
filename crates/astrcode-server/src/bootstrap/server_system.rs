//! Server 核心系统组装 — 事件总线 + scheduler + handler actor。

use std::sync::Arc;

use astrcode_protocol::events::ClientNotification;
use astrcode_support::event_fanout::EventFanout;

use super::ServerRuntime;
use crate::{
    handler::CommandHandle, server_event_bus::ServerEventBus, turn_scheduler::TurnScheduler,
};

/// Server 核心系统句柄。
///
/// 封装事件总线、scheduler、handler actor 等共享组件的初始化，
/// 保证各传输层入口（stdio / in-process / ACP / HTTP）的组装顺序一致。
pub struct ServerSystem {
    /// 事件广播发送端，传输层用它订阅事件。
    pub event_tx: Arc<EventFanout<ClientNotification>>,
    /// 事件总线，传输层用它发送非 session 通知。
    pub event_bus: Arc<ServerEventBus>,
    /// 命令处理句柄，传输层用它发送命令。
    pub handler: CommandHandle,
    /// Turn 调度器，共享给 CommandHandler 和 SessionOperations。
    pub scheduler: Arc<TurnScheduler>,
}

/// 组装 server 核心组件：创建事件总线 → 创建 scheduler → 绑定 session ops → 启动 handler actor。
///
/// `event_tx` 由调用方创建并传入，传输层可保留自己的订阅端。
pub fn spawn_server_system(
    runtime: &Arc<ServerRuntime>,
    event_tx: Arc<EventFanout<ClientNotification>>,
) -> ServerSystem {
    let scheduler = Arc::clone(runtime.scheduler());

    let event_bus = Arc::new(ServerEventBus::new(
        Arc::clone(&event_tx),
        Arc::clone(&scheduler),
    ));

    runtime
        .session_manager()
        .bind_event_bus(Arc::clone(&event_bus));

    let handler = CommandHandle::spawn(
        Arc::clone(runtime),
        Arc::clone(&scheduler),
        Arc::clone(&event_bus),
    );

    ServerSystem {
        event_tx,
        event_bus,
        handler,
        scheduler,
    }
}

//! Server 核心系统组装 — 事件总线挂载 + handler actor 启动。

use std::sync::Arc;

use astrcode_protocol::events::ClientNotification;
use astrcode_support::event_fanout::EventFanout;

use super::ServerRuntime;
use crate::{handler::CommandHandle, server_event_bus::ServerEventBus};

/// Server 核心系统句柄。
///
/// 封装事件总线、handler actor 等共享组件的初始化，
/// 保证各传输层入口（stdio / in-process / ACP）的组装顺序一致。
pub struct ServerSystem {
    /// 事件广播发送端，传输层用它订阅事件。
    pub event_tx: Arc<EventFanout<ClientNotification>>,
    /// 命令处理句柄，传输层用它发送命令。
    pub handler: CommandHandle,
}

/// 组装 server 核心组件：创建事件总线 → 注入 session attach hook → 启动 handler actor。
///
/// `event_tx` 由调用方创建并传入，传输层可保留自己的订阅端。
pub fn spawn_server_system(
    runtime: &Arc<ServerRuntime>,
    event_tx: Arc<EventFanout<ClientNotification>>,
) -> ServerSystem {
    let event_bus = Arc::new(ServerEventBus::new(
        runtime.event_store.clone(),
        Arc::clone(&event_tx),
    ));
    {
        let event_bus = Arc::clone(&event_bus);
        runtime
            .session_manager
            .set_attach_hook(Arc::new(move |session| {
                event_bus.attach(session);
            }));
    }
    let handler = CommandHandle::spawn(Arc::clone(runtime), Arc::clone(&event_bus));

    ServerSystem { event_tx, handler }
}

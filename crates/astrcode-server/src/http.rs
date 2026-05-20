//! Axum HTTP/SSE 入口。
//!
//! 这层只做 wire 适配：命令统一进入 [`crate::handler::CommandHandler`]，读接口从
//! storage read model 映射到 `astrcode_protocol::http` DTO。

use std::sync::Arc;

use astrcode_protocol::http::ConversationErrorEnvelopeDto;
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::bootstrap::ServerRuntime;

mod auth;
mod projection;
mod routes;
mod server;
mod stream;

pub use auth::ASTRCODE_HTTP_TOKEN_ENV;
pub use server::{HttpServerError, remove_run_info, router, run_http_server, write_run_info};

/// HTTP router shared state.
#[derive(Clone)]
pub(crate) struct HttpState {
    pub(crate) runtime: Arc<ServerRuntime>,
    pub(crate) handler: crate::handler::CommandHandle,
    pub(crate) event_bus: Arc<crate::server_event_bus::ServerEventBus>,
}

pub(crate) fn error_response(
    status: StatusCode,
    code: impl Into<String>,
    message: impl ToString,
) -> Response {
    (
        status,
        Json(ConversationErrorEnvelopeDto {
            code: code.into(),
            message: message.to_string(),
        }),
    )
        .into_response()
}

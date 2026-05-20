//! HTTP 鉴权中间件、Bearer token 加载与 CORS 来源收集。

use axum::{
    extract::State,
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::Response,
};

use super::error_response;

pub const ASTRCODE_HTTP_TOKEN_ENV: &str = "ASTRCODE_HTTP_TOKEN";

pub(super) async fn auth_middleware(
    State(expected_bearer): State<String>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let auth = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    match auth {
        Some(v) if v == expected_bearer => next.run(request).await,
        _ => error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Invalid or missing auth token",
        ),
    }
}

fn generate_auth_token() -> Result<String, getrandom::Error> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)?;

    Ok(bytes.iter().map(|b| format!("{:02x}", b)).collect())
}

pub(super) fn configured_auth_token() -> Result<String, getrandom::Error> {
    std::env::var(ASTRCODE_HTTP_TOKEN_ENV)
        .ok()
        .filter(|token| !token.trim().is_empty())
        .map(Ok)
        .unwrap_or_else(generate_auth_token)
}

pub(super) fn collect_allowed_origins() -> Vec<HeaderValue> {
    let mut origins = vec![
        "http://localhost:5173",
        "http://localhost:3000",
        "http://127.0.0.1:5173",
        "http://127.0.0.1:3000",
    ]
    .into_iter()
    .filter_map(|s| s.parse::<HeaderValue>().ok())
    .collect::<Vec<_>>();
    if let Ok(extra) = std::env::var("ASTRCODE_CORS_ORIGINS") {
        for origin in extra.split(',') {
            if let Ok(hv) = origin.trim().parse::<HeaderValue>() {
                origins.push(hv);
            }
        }
    }
    origins
}

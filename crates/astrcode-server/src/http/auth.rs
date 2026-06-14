//! HTTP 鉴权中间件、Bearer token 加载与 CORS 来源收集。

use axum::{
    extract::State,
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::Response,
};
use uuid::Uuid;

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

fn generate_auth_token() -> String {
    Uuid::new_v4().simple().to_string()
}

pub(super) fn configured_auth_token() -> String {
    std::env::var(ASTRCODE_HTTP_TOKEN_ENV)
        .ok()
        .filter(|token| !token.trim().is_empty())
        .unwrap_or_else(generate_auth_token)
}

pub(super) fn collect_allowed_origins() -> Vec<HeaderValue> {
    collect_allowed_origins_from_extra(std::env::var("ASTRCODE_CORS_ORIGINS").ok().as_deref())
}

fn collect_allowed_origins_from_extra(extra: Option<&str>) -> Vec<HeaderValue> {
    let mut origins = vec![
        "http://localhost:5173",
        "http://localhost:3000",
        "http://127.0.0.1:5173",
        "http://127.0.0.1:3000",
    ]
    .into_iter()
    .filter_map(|s| s.parse::<HeaderValue>().ok())
    .collect::<Vec<_>>();

    if let Some(extra) = extra {
        for origin in extra.split(',') {
            if let Ok(hv) = origin.trim().parse::<HeaderValue>() {
                origins.push(hv);
            }
        }
    }
    origins
}

#[cfg(test)]
mod tests {
    use super::collect_allowed_origins_from_extra;

    fn origin_strings(extra: Option<&str>) -> Vec<String> {
        collect_allowed_origins_from_extra(extra)
            .into_iter()
            .filter_map(|value| value.to_str().ok().map(str::to_owned))
            .collect()
    }

    #[test]
    fn default_allowed_origins_are_web_dev_hosts_only() {
        let origins = origin_strings(None);

        assert!(origins.contains(&"http://localhost:5173".to_string()));
        assert!(origins.contains(&"http://127.0.0.1:5173".to_string()));
        assert!(!origins.contains(&"http://tauri.localhost".to_string()));
        assert!(!origins.contains(&"https://tauri.localhost".to_string()));
    }

    #[test]
    fn explicit_allowed_origins_are_appended() {
        let origins = origin_strings(Some(
            "https://example.test, bad\norigin, https://admin.example.test",
        ));

        assert!(origins.contains(&"https://example.test".to_string()));
        assert!(origins.contains(&"https://admin.example.test".to_string()));
        assert!(!origins.contains(&"bad\norigin".to_string()));
    }
}

use std::time::Duration;

use async_trait::async_trait;
use reqwest::{
    StatusCode,
    header::{ACCEPT, CONTENT_TYPE},
};
use serde_json::Value;

use crate::{
    client::{McpClient, McpClientError},
    config::McpServerConfig,
    protocol::{self, CallToolResult, McpTool},
};

const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const CONTENT_TYPE_JSON: &str = "application/json";
const CONTENT_TYPE_EVENT_STREAM: &str = "text/event-stream";
const MCP_PROTOCOL_VERSION_HEADER: &str = "MCP-Protocol-Version";
const MCP_SESSION_ID_HEADER: &str = "Mcp-Session-Id";

pub(crate) struct HttpMcpClient {
    url: String,
    headers: Vec<(String, String)>,
    client: reqwest::Client,
    timeout: Duration,
}

impl HttpMcpClient {
    pub(crate) fn new(server: McpServerConfig) -> Self {
        let url = server.url.expect("HttpMcpClient requires url");
        let headers = server
            .headers
            .into_iter()
            .collect::<Vec<(String, String)>>();
        Self {
            url,
            headers,
            client: reqwest::Client::new(),
            timeout: DEFAULT_HTTP_TIMEOUT,
        }
    }

    #[cfg(test)]
    fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    async fn initialize(&self) -> Result<HttpMcpSession, McpClientError> {
        let response = self
            .post_request(protocol::initialize_request(1), 1, None)
            .await?;
        let initialize =
            protocol::parse_initialize(response.result).map_err(McpClientError::Result)?;
        let protocol_version = initialize
            .protocol_version
            .unwrap_or_else(|| protocol::MCP_PROTOCOL_VERSION.to_string());
        let session = HttpMcpSession {
            session_id: response.session_id,
            protocol_version,
        };
        self.post_notification(protocol::initialized_notification(), Some(&session))
            .await?;
        Ok(session)
    }

    async fn post_request(
        &self,
        body: Value,
        expected_id: u64,
        session: Option<&HttpMcpSession>,
    ) -> Result<HttpRpcResult, McpClientError> {
        let response = self.send(body, session).await?;
        let session_id = response.session_id;
        let result = parse_response_body(
            &self.url,
            response.status,
            response.content_type.as_deref(),
            response.body.as_str(),
            ResponseKind::Request { expected_id },
        )?;
        Ok(HttpRpcResult { result, session_id })
    }

    async fn post_notification(
        &self,
        body: Value,
        session: Option<&HttpMcpSession>,
    ) -> Result<(), McpClientError> {
        let response = self.send(body, session).await?;
        parse_response_body(
            &self.url,
            response.status,
            response.content_type.as_deref(),
            response.body.as_str(),
            ResponseKind::Notification,
        )?;
        Ok(())
    }

    async fn send(
        &self,
        body: Value,
        session: Option<&HttpMcpSession>,
    ) -> Result<HttpResponseBody, McpClientError> {
        let mut request = self
            .client
            .post(&self.url)
            .timeout(self.timeout)
            .json(&body);

        for (key, value) in &self.headers {
            request = request.header(key.as_str(), value.as_str());
        }
        request = request
            .header(CONTENT_TYPE, CONTENT_TYPE_JSON)
            .header(
                ACCEPT,
                format!("{CONTENT_TYPE_JSON}, {CONTENT_TYPE_EVENT_STREAM}"),
            )
            .header(
                MCP_PROTOCOL_VERSION_HEADER,
                session
                    .map(|session| session.protocol_version.as_str())
                    .unwrap_or(protocol::MCP_PROTOCOL_VERSION),
            );
        if let Some(session_id) = session.and_then(|session| session.session_id.as_deref()) {
            request = request.header(MCP_SESSION_ID_HEADER, session_id);
        }

        let response = request.send().await.map_err(|source| {
            if source.is_timeout() {
                McpClientError::HttpTimeout {
                    url: self.url.clone(),
                }
            } else {
                McpClientError::Http {
                    message: format!("send request to {}: {source}", self.url),
                }
            }
        })?;

        let status = response.status();
        let session_id = response
            .headers()
            .get(MCP_SESSION_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string);
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let response_text = response
            .text()
            .await
            .map_err(|source| McpClientError::Http {
                message: format!("read response body: {source}"),
            })?;

        Ok(HttpResponseBody {
            status,
            content_type,
            session_id,
            body: response_text,
        })
    }
}

#[derive(Debug)]
struct HttpMcpSession {
    session_id: Option<String>,
    protocol_version: String,
}

struct HttpRpcResult {
    result: Value,
    session_id: Option<String>,
}

struct HttpResponseBody {
    status: StatusCode,
    content_type: Option<String>,
    session_id: Option<String>,
    body: String,
}

#[derive(Debug, Clone, Copy)]
enum ResponseKind {
    Request { expected_id: u64 },
    Notification,
}

fn parse_response_body(
    url: &str,
    status: StatusCode,
    content_type: Option<&str>,
    body: &str,
    kind: ResponseKind,
) -> Result<Value, McpClientError> {
    if matches!(kind, ResponseKind::Notification) && status == StatusCode::ACCEPTED {
        return Ok(Value::Null);
    }
    if !status.is_success() {
        return Err(McpClientError::Http {
            message: format!("HTTP {status} from {url}; body: {body}"),
        });
    }
    if body.trim().is_empty() {
        return match kind {
            ResponseKind::Notification => Ok(Value::Null),
            ResponseKind::Request { .. } => Err(McpClientError::Http {
                message: format!("empty JSON-RPC response body from {url}"),
            }),
        };
    }

    match content_type.map(str::to_ascii_lowercase) {
        Some(content_type) if content_type.starts_with(CONTENT_TYPE_EVENT_STREAM) => {
            parse_sse_response(url, body, kind)
        },
        _ => parse_json_response(url, body, kind),
    }
}

fn parse_json_response(url: &str, body: &str, kind: ResponseKind) -> Result<Value, McpClientError> {
    let rpc_response: protocol::JsonRpcResponse =
        serde_json::from_str(body).map_err(|source| McpClientError::Http {
            message: format!("parse JSON-RPC response from {url}: {source}; body: {body}"),
        })?;
    response_result(rpc_response, kind)
}

fn parse_sse_response(url: &str, body: &str, kind: ResponseKind) -> Result<Value, McpClientError> {
    let mut data_lines = Vec::new();
    for line in body.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            if let Some(result) = parse_sse_event(url, &data_lines, kind)? {
                return Ok(result);
            }
            data_lines.clear();
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start().to_string());
        }
    }
    if let Some(result) = parse_sse_event(url, &data_lines, kind)? {
        return Ok(result);
    }

    Err(McpClientError::Http {
        message: format!("SSE response from {url} did not contain the expected JSON-RPC response"),
    })
}

fn parse_sse_event(
    url: &str,
    data_lines: &[String],
    kind: ResponseKind,
) -> Result<Option<Value>, McpClientError> {
    if data_lines.is_empty() {
        return Ok(None);
    }
    let data = data_lines.join("\n");
    let rpc_response: protocol::JsonRpcResponse =
        serde_json::from_str(&data).map_err(|source| McpClientError::Http {
            message: format!("parse SSE JSON-RPC response from {url}: {source}; data: {data}"),
        })?;

    match kind {
        ResponseKind::Request { expected_id } if rpc_response.id != Some(expected_id) => Ok(None),
        _ => response_result(rpc_response, kind).map(Some),
    }
}

fn response_result(
    rpc_response: protocol::JsonRpcResponse,
    kind: ResponseKind,
) -> Result<Value, McpClientError> {
    if let Some(error) = rpc_response.error {
        return Err(McpClientError::Rpc {
            code: error.code,
            message: error.message,
            stderr: String::new(),
        });
    }
    if let ResponseKind::Request { expected_id } = kind {
        let Some(actual_id) = rpc_response.id else {
            return Err(McpClientError::MismatchedResponse {
                expected: expected_id,
                actual: None,
                stderr: String::new(),
            });
        };
        if actual_id != expected_id {
            return Err(McpClientError::MismatchedResponse {
                expected: expected_id,
                actual: Some(actual_id),
                stderr: String::new(),
            });
        }
    }
    Ok(rpc_response.result.unwrap_or(Value::Null))
}

#[async_trait]
impl McpClient for HttpMcpClient {
    async fn list_tools(&self) -> Result<Vec<McpTool>, McpClientError> {
        let session = self.initialize().await?;
        let result = self
            .post_request(protocol::list_tools_request(2), 2, Some(&session))
            .await?
            .result;
        protocol::parse_list_tools(result).map_err(McpClientError::Result)
    }

    async fn call_tool(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> Result<CallToolResult, McpClientError> {
        let session = self.initialize().await?;
        let result = self
            .post_request(
                protocol::call_tool_request(2, tool_name, arguments),
                2,
                Some(&session),
            )
            .await?
            .result;
        protocol::parse_call_tool(result).map_err(McpClientError::Result)
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc, time::Duration};

    use serde_json::json;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::Mutex,
    };

    use super::*;
    use crate::config::McpTransport;

    #[tokio::test]
    async fn lists_tools_over_http_with_session_and_accepted_notification() {
        let server = TestHttpServer::start(vec![
            TestResponse::json(
                StatusCode::OK,
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "protocolVersion": "2025-06-18",
                        "capabilities": {},
                        "serverInfo": {"name": "fake", "version": "1"}
                    }
                }),
            )
            .header(MCP_SESSION_ID_HEADER, "session-1"),
            TestResponse::empty(StatusCode::ACCEPTED),
            TestResponse::json(
                StatusCode::OK,
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "result": {
                        "tools": [{
                            "name": "echo",
                            "description": "Echo text",
                            "inputSchema": {"type": "object"}
                        }]
                    }
                }),
            ),
        ])
        .await;
        let client = HttpMcpClient::new(server_config(
            server.url(),
            BTreeMap::from([("Authorization".into(), "Bearer test".into())]),
        ))
        .with_timeout(Duration::from_secs(5));

        let tools = client.list_tools().await.unwrap();

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
        let requests = server.requests().await;
        assert_eq!(requests.len(), 3);
        assert!(requests[0].body.contains("\"method\":\"initialize\""));
        assert!(requests[1].body.contains("\"notifications/initialized\""));
        assert_eq!(
            requests[1]
                .headers
                .get("mcp-session-id")
                .map(String::as_str),
            Some("session-1")
        );
        assert_eq!(
            requests[2]
                .headers
                .get("mcp-session-id")
                .map(String::as_str),
            Some("session-1")
        );
        assert_eq!(
            requests[2].headers.get("authorization").map(String::as_str),
            Some("Bearer test")
        );
        let accept = requests[2].headers.get("accept").unwrap();
        assert!(accept.contains(CONTENT_TYPE_JSON));
        assert!(accept.contains(CONTENT_TYPE_EVENT_STREAM));
    }

    #[tokio::test]
    async fn lists_tools_from_http_sse_response() {
        let server = TestHttpServer::start(vec![
            TestResponse::json(
                StatusCode::OK,
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {"protocolVersion": "2025-06-18"}
                }),
            ),
            TestResponse::empty(StatusCode::ACCEPTED),
            TestResponse::sse(
                StatusCode::OK,
                format!(
                    "data: {}\n\ndata: {}\n\n",
                    json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/progress",
                        "params": {}
                    }),
                    json!({
                        "jsonrpc": "2.0",
                        "id": 2,
                        "result": {"tools": [{"name": "search"}]}
                    })
                ),
            ),
        ])
        .await;
        let client = HttpMcpClient::new(server_config(server.url(), BTreeMap::new()))
            .with_timeout(Duration::from_secs(5));

        let tools = client.list_tools().await.unwrap();

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "search");
    }

    #[tokio::test]
    async fn calls_tool_over_http() {
        let server = TestHttpServer::start(vec![
            TestResponse::json(
                StatusCode::OK,
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {"protocolVersion": "2025-06-18"}
                }),
            ),
            TestResponse::empty(StatusCode::ACCEPTED),
            TestResponse::json(
                StatusCode::OK,
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "result": {
                        "content": [{"type": "text", "text": "called"}],
                        "isError": false
                    }
                }),
            ),
        ])
        .await;
        let client = HttpMcpClient::new(server_config(server.url(), BTreeMap::new()))
            .with_timeout(Duration::from_secs(5));

        let result = client
            .call_tool("echo", json!({"text": "hi"}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(protocol::render_call_content(&result), "called");
        let requests = server.requests().await;
        assert!(requests[2].body.contains("\"tools/call\""));
        assert!(requests[2].body.contains("\"name\":\"echo\""));
    }

    fn server_config(url: String, headers: BTreeMap<String, String>) -> McpServerConfig {
        McpServerConfig {
            name: "http-fake".into(),
            transport: McpTransport::Http,
            command: String::new(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            url: Some(url),
            headers,
        }
    }

    struct TestHttpServer {
        addr: std::net::SocketAddr,
        requests: Arc<Mutex<Vec<TestRequest>>>,
    }

    impl TestHttpServer {
        async fn start(responses: Vec<TestResponse>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let server_requests = Arc::clone(&requests);
            tokio::spawn(async move {
                for response in responses {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let request = read_request(&mut socket).await;
                    server_requests.lock().await.push(request);
                    socket
                        .write_all(response.to_http().as_bytes())
                        .await
                        .unwrap();
                }
            });
            Self { addr, requests }
        }

        fn url(&self) -> String {
            format!("http://{}/mcp", self.addr)
        }

        async fn requests(&self) -> Vec<TestRequest> {
            self.requests.lock().await.clone()
        }
    }

    #[derive(Clone, Debug)]
    struct TestRequest {
        headers: BTreeMap<String, String>,
        body: String,
    }

    struct TestResponse {
        status: StatusCode,
        headers: BTreeMap<String, String>,
        body: String,
    }

    impl TestResponse {
        fn json(status: StatusCode, body: Value) -> Self {
            Self {
                status,
                headers: BTreeMap::from([(CONTENT_TYPE.as_str().into(), CONTENT_TYPE_JSON.into())]),
                body: body.to_string(),
            }
        }

        fn sse(status: StatusCode, body: impl Into<String>) -> Self {
            Self {
                status,
                headers: BTreeMap::from([(
                    CONTENT_TYPE.as_str().into(),
                    CONTENT_TYPE_EVENT_STREAM.into(),
                )]),
                body: body.into(),
            }
        }

        fn empty(status: StatusCode) -> Self {
            Self {
                status,
                headers: BTreeMap::new(),
                body: String::new(),
            }
        }

        fn header(mut self, key: &str, value: &str) -> Self {
            self.headers.insert(key.into(), value.into());
            self
        }

        fn to_http(&self) -> String {
            let reason = self.status.canonical_reason().unwrap_or("");
            let mut response = format!(
                "HTTP/1.1 {} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n",
                self.status.as_u16(),
                self.body.len()
            );
            for (key, value) in &self.headers {
                response.push_str(key);
                response.push_str(": ");
                response.push_str(value);
                response.push_str("\r\n");
            }
            response.push_str("\r\n");
            response.push_str(&self.body);
            response
        }
    }

    async fn read_request(socket: &mut tokio::net::TcpStream) -> TestRequest {
        let mut bytes = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let n = socket.read(&mut buf).await.unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buf[..n]);
            if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }

        let header_end = bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap()
            + 4;
        let headers_text = String::from_utf8_lossy(&bytes[..header_end]);
        let headers = parse_headers(&headers_text);
        let content_length = headers
            .get("content-length")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        while bytes.len() - header_end < content_length {
            let n = socket.read(&mut buf).await.unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buf[..n]);
        }
        let body =
            String::from_utf8_lossy(&bytes[header_end..header_end + content_length]).to_string();

        TestRequest { headers, body }
    }

    fn parse_headers(headers_text: &str) -> BTreeMap<String, String> {
        headers_text
            .lines()
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_string()))
            .collect()
    }
}

//! 协议版本协商模块。
//!
//! 定义客户端/服务器握手时的版本交换类型，
//! 以及版本协商算法（选择双方都支持的最高版本）。

use serde::{Deserialize, Serialize};

/// 客户端发起的初始化握手请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeRequest {
    pub protocol_version: u32,
    pub client_info: ClientInfo,
}

/// 服务器对初始化请求的响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeResponse {
    pub accepted_version: u32,
    pub server_info: ServerInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    /// 如 `astrcode-web`。
    pub name: String,
    /// 语义化版本字符串。
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
    pub protocol_versions: Vec<u32>,
    pub capabilities: ServerCapabilities,
}

/// 服务器能力标志集合。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServerCapabilities {
    pub streaming: bool,
    pub session_fork: bool,
    pub compaction: bool,
    pub extensions: bool,
}

/// 在客户端和服务器之间协商协议版本。
///
/// 优先返回客户端请求的版本；若服务器不支持该版本，
/// 则返回双方都支持的最高版本；若完全不兼容则返回 `None`。
pub fn negotiate_version(client_requested: u32, server_supported: &[u32]) -> Option<u32> {
    server_supported
        .iter()
        .copied()
        .filter(|&v| v <= client_requested)
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_negotiate_exact_match() {
        let result = negotiate_version(1, &[1, 2]);
        assert_eq!(result, Some(1));
    }

    #[test]
    fn test_negotiate_highest_compatible() {
        let result = negotiate_version(3, &[1, 2]);
        assert_eq!(result, Some(2));
    }

    #[test]
    fn test_negotiate_incompatible() {
        let result = negotiate_version(1, &[2, 3]);
        assert_eq!(result, None);
    }
}

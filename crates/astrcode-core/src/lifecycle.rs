//! session 生命周期相关的 trait。
//!
//! 定义 session 销毁/回收时需要清理的外部资源的注入接口，
//! 避免上层直接依赖具体资源的实现细节。

use crate::types::SessionId;

/// session 销毁或回收时需要清理的外部进程资源。
///
/// 实现必须是幂等的——同一 session 多次调用 cleanup 应安全无副作用。
pub trait SessionResourceCleanup: Send + Sync {
    /// 清理指定 session 关联的所有外部资源。
    fn cleanup(&self, session_id: &SessionId);
}

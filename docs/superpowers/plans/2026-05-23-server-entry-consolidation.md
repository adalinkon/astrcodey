# Server 入口去冗余 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 消除 astrcode-server 三处入口（HTTP 二进制、stdio 主入口、in-process 传输、ACP 适配器）的重复组装代码和硬编码标记检测。

**Architecture:** 三步走——先提取共享的 compact summary 文本检测函数，再让 HTTP 二进制委托给已有的 `run_http_server`，最后在 bootstrap 中封装通用的 server system 启动函数。

**Tech Stack:** Rust, tokio, axum

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `crates/astrcode-context/src/compaction/mod.rs` | 导出 `COMPACT_SUMMARY_MARKER` 常量 + `is_compact_summary_text(&str)` 函数 |
| Modify | `crates/astrcode-server/src/handler/snapshot.rs` | 删除本地 `is_compact_summary_message`，改用 `astrcode_context::compaction::is_compact_summary_text` |
| Modify | `crates/astrcode-cli/src/tui/app/handle_event.rs` | 同上 |
| Modify | `crates/astrcode-server/Cargo.toml` | 确认已有 `astrcode-context` 依赖 |
| Modify | `crates/astrcode-cli/Cargo.toml` | 添加 `astrcode-context` 依赖 |
| Modify | `crates/astrcode-server/src/http_main.rs` | 委托给 `run_http_server`，删除重复代码 |
| Modify | `crates/astrcode-server/src/http/server.rs` | 删除关于"同步两处"的过时注释 |
| Create | `crates/astrcode-server/src/bootstrap/server_system.rs` | `spawn_server_system` 函数 |
| Modify | `crates/astrcode-server/src/bootstrap.rs` | 注册新模块，导出 `spawn_server_system` |
| Modify | `crates/astrcode-server/src/main.rs` | 使用 `spawn_server_system` |
| Modify | `crates/astrcode-cli/src/transport.rs` | 使用 `spawn_server_system` |
| Modify | `crates/astrcode-server/src/acp/mod.rs` | 使用 `spawn_server_system` |

---

### Task 1: 提取 `is_compact_summary_text` 到 `astrcode-context`

**Files:**
- Modify: `crates/astrcode-context/src/compaction/mod.rs:20,297-306`

- [ ] **Step 1: 在 `astrcode-context/src/compaction/mod.rs` 中添加 `is_compact_summary_text` 函数并导出常量**

在 `COMPACT_SUMMARY_MARKER` 常量（第 20 行）下方添加 `pub` 使其公开：

```rust
pub const COMPACT_SUMMARY_MARKER: &str = "<compact_summary>";
```

在 `is_compact_summary_message` 函数（第 297 行）之后添加新函数：

```rust
/// 检测文本内容是否以 compact summary 标记开头。
///
/// 用于只持有序列化文本的客户端（如 TUI、snapshot DTO 转换），
/// 不依赖 `LlmMessage` 结构化类型。
pub fn is_compact_summary_text(content: &str) -> bool {
    content.trim_start().starts_with(COMPACT_SUMMARY_MARKER)
}
```

- [ ] **Step 2: 运行现有测试确认无回归**

Run: `cargo test -p astrcode-context --all-features`
Expected: 所有测试通过

- [ ] **Step 3: Commit**

```bash
git add crates/astrcode-context/src/compaction/mod.rs
git commit -m "feat(context): 导出 COMPACT_SUMMARY_MARKER 常量及 is_compact_summary_text 函数"
```

---

### Task 2: 消除 `snapshot.rs` 中的本地 `is_compact_summary_message`

**Files:**
- Modify: `crates/astrcode-server/src/handler/snapshot.rs:55,67-70`

- [ ] **Step 1: 替换 `snapshot.rs` 中的本地函数调用**

将第 55 行的调用：

```rust
let role = if is_compact_summary_message(&content) {
```

改为：

```rust
let role = if astrcode_context::compaction::is_compact_summary_text(&content) {
```

删除第 67-70 行的本地函数定义：

```rust
// 删除这整段
fn is_compact_summary_message(content: &str) -> bool {
    content.trim_start().starts_with("<compact_summary>")
}
```

- [ ] **Step 2: 更新 `snapshot.rs` 底部的测试**

将测试函数 `is_compact_summary_message_detects_marker`（约第 140-146 行）中对本地函数的调用改为调用新函数：

```rust
#[test]
fn is_compact_summary_message_detects_marker() {
    use astrcode_context::compaction::is_compact_summary_text;
    assert!(is_compact_summary_text("<compact_summary>\nContent"));
    assert!(is_compact_summary_text("  <compact_summary>\nContent"));
    assert!(is_compact_summary_text("\n<compact_summary>\nContent"));
    assert!(!is_compact_summary_text("Regular message"));
    assert!(!is_compact_summary_text("</compact_summary>"));
}
```

- [ ] **Step 3: 运行测试确认通过**

Run: `cargo test -p astrcode-server --all-features`
Expected: 所有测试通过

- [ ] **Step 4: Commit**

```bash
git add crates/astrcode-server/src/handler/snapshot.rs
git commit -m "refactor(server): snapshot.rs 使用 astrcode-context 的 is_compact_summary_text"
```

---

### Task 3: 消除 `handle_event.rs` 中的本地 `is_compact_summary_message`

**Files:**
- Modify: `crates/astrcode-cli/src/tui/app/handle_event.rs:593,611-614`
- Modify: `crates/astrcode-cli/Cargo.toml`

- [ ] **Step 1: 在 `astrcode-cli/Cargo.toml` 中添加 `astrcode-context` 依赖**

在 `[dependencies]` 部分添加：

```toml
astrcode-context = { path = "../astrcode-context" }
```

- [ ] **Step 2: 替换 `handle_event.rs` 中的本地函数调用**

将第 593 行：

```rust
let label = if is_compact_summary_message(&message.content) {
```

改为：

```rust
let label = if astrcode_context::compaction::is_compact_summary_text(&message.content) {
```

删除第 611-614 行的本地函数定义：

```rust
// 删除这整段
fn is_compact_summary_message(content: &str) -> bool {
    content.trim_start().starts_with("<compact_summary>")
}
```

- [ ] **Step 3: 运行测试确认通过**

Run: `cargo test -p astrcode-cli --all-features`
Expected: 编译通过、测试通过

- [ ] **Step 4: Commit**

```bash
git add crates/astrcode-cli/Cargo.toml crates/astrcode-cli/src/tui/app/handle_event.rs
git commit -m "refactor(cli): handle_event.rs 使用 astrcode-context 的 is_compact_summary_text"
```

---

### Task 4: 重构 `http_main.rs` 委托给 `run_http_server`

**Files:**
- Modify: `crates/astrcode-server/src/http_main.rs`
- Modify: `crates/astrcode-server/src/http/server.rs:131-141`

- [ ] **Step 1: 重写 `http_main.rs`**

将整个文件替换为：

```rust
//! HTTP/SSE server binary.
//!
//! stdio JSON-RPC remains the default `astrcode-server` binary; this entry
//! starts the additive HTTP surface.

#![windows_subsystem = "windows"]

use std::{net::SocketAddr, sync::Arc};

#[tokio::main]
async fn main() {
    let _guard = astrcode_log::init();
    tracing::info!("astrcode-http-server starting");

    let runtime = match astrcode_server::bootstrap::bootstrap().await {
        Ok(rt) => Arc::new(rt),
        Err(error) => {
            tracing::error!("Bootstrap failed: {error}");
            std::process::exit(1);
        },
    };

    let addr: SocketAddr = std::env::var("ASTRCODE_HTTP_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:3847".into())
        .parse()
        .unwrap_or_else(|error| {
            tracing::error!("Invalid ASTRCODE_HTTP_ADDR: {error}");
            std::process::exit(1);
        });

    if let Err(error) = astrcode_server::http::run_http_server(runtime, addr).await {
        tracing::error!("HTTP server failed: {error}");
        std::process::exit(1);
    }
}
```

- [ ] **Step 2: 删除 `server.rs` 中关于"同步两处"的过时注释**

删除 `run_http_server` 函数上方的注释块（第 131-141 行），保留函数文档的第一行即可：

```rust
/// Convenience wrapper: build router and run until graceful shutdown.
pub async fn run_http_server(
```

- [ ] **Step 3: 运行测试确认通过**

Run: `cargo test -p astrcode-server --all-features`
Expected: 所有测试通过

- [ ] **Step 4: Commit**

```bash
git add crates/astrcode-server/src/http_main.rs crates/astrcode-server/src/http/server.rs
git commit -m "refactor(server): http_main.rs 委托给 run_http_server，消除重复网络/停机代码"
```

---

### Task 5: 创建 `spawn_server_system` 公共函数

**Files:**
- Create: `crates/astrcode-server/src/bootstrap/server_system.rs`
- Modify: `crates/astrcode-server/src/bootstrap.rs`

- [ ] **Step 1: 创建 `crates/astrcode-server/src/bootstrap/server_system.rs`**

```rust
//! Server 核心系统组装 — 事件总线挂载 + handler actor 启动。

use std::sync::Arc;

use astrcode_protocol::events::ClientNotification;
use astrcode_support::event_fanout::EventFanout;

use super::ServerRuntime;
use crate::{
    handler::CommandHandle,
    server_event_bus::ServerEventBus,
};

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
```

- [ ] **Step 2: 在 `bootstrap.rs` 中注册模块并导出**

在 `bootstrap.rs` 的模块声明区域添加：

```rust
mod server_system;

pub use server_system::{ServerSystem, spawn_server_system};
```

- [ ] **Step 3: 运行编译确认无错误**

Run: `cargo check -p astrcode-server`
Expected: 编译通过

- [ ] **Step 4: Commit**

```bash
git add crates/astrcode-server/src/bootstrap/server_system.rs crates/astrcode-server/src/bootstrap.rs
git commit -m "feat(server): 添加 spawn_server_system 统一核心组件组装"
```

---

### Task 6: 迁移 `main.rs`（stdio 入口）使用 `spawn_server_system`

**Files:**
- Modify: `crates/astrcode-server/src/main.rs:64-78`

- [ ] **Step 1: 替换 `main.rs` 中的手动组装代码**

将第 64-78 行：

```rust
let event_tx = Arc::new(EventFanout::new());

let event_bus = Arc::new(astrcode_server::server_event_bus::ServerEventBus::new(
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
let handler = CommandHandler::spawn_actor(Arc::clone(&runtime), Arc::clone(&event_bus));
```

替换为：

```rust
let event_tx = Arc::new(EventFanout::new());
let server_system =
    astrcode_server::bootstrap::spawn_server_system(&runtime, Arc::clone(&event_tx));
let handler = server_system.handler;
```

同时清理未使用的 import：删除 `CommandHandler` 和 `server_event_bus` 的 use 行（如果有），只保留 `EventFanout`。

- [ ] **Step 2: 运行测试确认通过**

Run: `cargo test -p astrcode-server --all-features`
Expected: 所有测试通过

- [ ] **Step 3: Commit**

```bash
git add crates/astrcode-server/src/main.rs
git commit -m "refactor(server): main.rs 使用 spawn_server_system 替代手动组装"
```

---

### Task 7: 迁移 `transport.rs`（in-process 入口）使用 `spawn_server_system`

**Files:**
- Modify: `crates/astrcode-cli/src/transport.rs:59-68`

- [ ] **Step 1: 替换 `transport.rs` 中的手动组装代码**

将第 59-68 行：

```rust
let event_bus = Arc::new(ServerEventBus::new(runtime.event_store.clone(), tx));
{
    let event_bus = Arc::clone(&event_bus);
    runtime
        .session_manager
        .set_attach_hook(Arc::new(move |session| {
            event_bus.attach(session);
        }));
}
let handler = CommandHandler::spawn_actor(runtime, event_bus);
```

替换为：

```rust
let server_system = bootstrap::spawn_server_system(&runtime, tx);
let handler = server_system.handler;
```

同时清理未使用的 import：删除 `CommandHandler`、`ServerEventBus` 的 use 行。

- [ ] **Step 2: 运行测试确认通过**

Run: `cargo test -p astrcode-cli --all-features`
Expected: 所有测试通过

- [ ] **Step 3: Commit**

```bash
git add crates/astrcode-cli/src/transport.rs
git commit -m "refactor(cli): transport.rs 使用 spawn_server_system 替代手动组装"
```

---

### Task 8: 迁移 `acp/mod.rs`（ACP 入口）使用 `spawn_server_system`

**Files:**
- Modify: `crates/astrcode-server/src/acp/mod.rs:35-48`

- [ ] **Step 1: 替换 `acp/mod.rs` 中的手动组装代码**

将第 35-48 行：

```rust
let event_tx = Arc::new(EventFanout::new());
let event_bus = Arc::new(crate::server_event_bus::ServerEventBus::new(
    runtime.event_store.clone(),
    event_tx,
));
{
    let event_bus = Arc::clone(&event_bus);
    runtime
        .session_manager
        .set_attach_hook(Arc::new(move |session| {
            event_bus.attach(session);
        }));
}
let command_handle = CommandHandle::spawn(Arc::clone(&runtime), Arc::clone(&event_bus));
```

替换为：

```rust
let event_tx = Arc::new(EventFanout::new());
let server_system =
    crate::bootstrap::spawn_server_system(&runtime, Arc::clone(&event_tx));
let command_handle = server_system.handler;
```

后续使用 `event_bus` 的地方（如 `handle_prompt` 中的 `event_bus.fanout().subscribe()`）需要更新为使用 `server_system` 返回的句柄。但由于 `spawn_server_system` 返回的 `ServerSystem` 没有直接暴露 `event_bus`，而 ACP 后续需要通过 `event_bus.fanout().subscribe()` 订阅事件，这里改为直接从 `event_tx` 订阅：

将第 144 行的：

```rust
let mut event_rx = event_bus.fanout().subscribe();
```

改为：

```rust
let mut event_rx = event_tx.subscribe();
```

同时删除 `acp/mod.rs` 中对 `event_bus` 的后续引用，将 `handle_prompt` 的签名中 `&Arc<ServerEventBus>` 参数替换为 `&Arc<EventFanout<ClientNotification>>`：

```rust
async fn handle_prompt(
    req: PromptRequest,
    command_handle: &CommandHandle,
    event_tx: &Arc<EventFanout<ClientNotification>>,
    cx: &ConnectionTo<Client>,
) -> Result<StopReason, Error> {
```

以及调用处（约第 98 行）的参数更新：

```rust
match handle_prompt(req, &command_handle, &event_tx, &cx).await {
```

清理未使用的 import：删除 `crate::server_event_bus::ServerEventBus` 和 `CommandHandle::spawn` 的 use。

- [ ] **Step 2: 运行测试确认通过**

Run: `cargo test -p astrcode-server --all-features`
Expected: 所有测试通过

- [ ] **Step 3: Commit**

```bash
git add crates/astrcode-server/src/acp/mod.rs
git commit -m "refactor(server): ACP 入口使用 spawn_server_system 替代手动组装"
```

---

### Task 9: 全局回归验证

- [ ] **Step 1: 运行完整 clippy 检查**

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: 无 warnings

- [ ] **Step 2: 运行完整测试套件**

Run: `cargo test --all-features`
Expected: 所有测试通过

- [ ] **Step 3: 确认 `http_main.rs` 编译为独立二进制**

Run: `cargo build -p astrcode-server --bin astrcode-http-server`
Expected: 编译成功

- [ ] **Step 4: Final commit (如有格式修正)**

```bash
cargo fmt
git add -A
git commit -m "chore: 格式化代码"  # 仅在有变更时提交
```

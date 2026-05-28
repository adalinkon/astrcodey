# 代码冗余清理

## [中] `HandlerError` 与 `TurnScheduleError` 变体重复

**位置**：
- `crates/astrcode-server/src/turn_scheduler.rs:46-61` — `TurnScheduleError`
- `crates/astrcode-server/src/handler/mod.rs:50-76` — `HandlerError`
- `crates/astrcode-server/src/handler/errors.rs` — 整个文件仅包含两者的机械 `From` impl

**问题**：`TurnAlreadyRunning`、`NoActiveTurn`、`SessionNotFound(String)` 三个变体在两个 enum 中逐字重复。`errors.rs` 是一个 26 行的专用文件，只做 1:1 无转换映射。

**方案**：消除 `TurnScheduleError`，让 `TurnScheduler` 直接返回 `HandlerError`；或者将三个共享变体提取为一个公共的 `TurnStateError` 子类型。

---

## [中] `KeybindingInfoDto` 与 core `Keybinding` 完全冗余

**位置**：
- `crates/astrcode-core/src/extension.rs:1365` — `Keybinding`（已有 `Serialize, Deserialize`）
- `crates/astrcode-protocol/src/events.rs:137` — `KeybindingInfoDto`（字段、类型、serde 行为完全一致，无 `rename_all`）
- `crates/astrcode-protocol/src/http.rs:92` — `KeybindingDto`（加了 `rename_all = "camelCase"`）

**问题**：`KeybindingInfoDto` 与 core 的 `Keybinding` 序列化行为完全相同——相同字段、相同类型、均无 `rename_all`。`KeybindingDto` 至少有 `camelCase` 的差异化，但 `KeybindingInfoDto` 纯属无意义拷贝。

**方案**：删除 `KeybindingInfoDto`，events 模块直接使用 core 的 `Keybinding`（或定义 type alias）。`KeybindingDto` 因 camelCase 差异保留，但可通过 `From<Keybinding>` impl 消除手动转换。

---

## [中] `AgentSessionStatusDto` 与 core `AgentSessionStatus` 序列化行为一致

**位置**：
- `crates/astrcode-core/src/storage.rs:261` — `AgentSessionStatus`（`rename_all = "snake_case"`）
- `crates/astrcode-protocol/src/agent_session_link.rs:12` — `AgentSessionStatusDto`（同样 `rename_all = "snake_case"`，变体 1:1）

**问题**：两个 enum 的 serde 行为完全相同，`From` impl 是纯机械映射，DTO 未提供任何额外的序列化格式隔离或字段裁剪。

**方案**：protocol 直接 re-export core 的 `AgentSessionStatus`（或用 `pub type AgentSessionStatusDto = AgentSessionStatus`），删除重复定义和 From impl。如果未来线缆格式需要与内部类型分化，再拆出 DTO。

---

## [低] `CompactResult` 名称碰撞

**位置**：
- `crates/astrcode-core/src/extension.rs:833` — `pub enum CompactResult { Allow, Block, Contributions }` — 扩展钩子结果
- `crcodes/astrcode-context/src/compaction/mod.rs:39` — `pub struct CompactResult { pre_tokens, post_tokens, ... }` — 压缩操作结果

**问题**：两个完全不同语义的类型共享 `CompactResult` 名称。`context::CompactResult` 被广泛引用（session、server handler），`extension::CompactResult` 在 extensions crate 内使用。虽然 Rust 的模块系统避免了冲突，但同名会造成阅读和维护时的混淆。

**方案**：重命名其中一个。建议将 `extension::CompactResult` 改为 `CompactHookResult`，语义更精确。

---

## [低] 6 个 extension crate 冗余 `crate-type = ["rlib"]`

**位置**：
- `crates/astrcode-extension-agent-tools/Cargo.toml:9`
- `crates/astrcode-extension-mcp/Cargo.toml:9`
- `crates/astrcode-extension-memory/Cargo.toml:9`
- `crates/astrcode-extension-mode/Cargo.toml:9`
- `crates/astrcode-extension-skill/Cargo.toml:9`
- `crates/astrcode-extension-todo-tool/Cargo.toml:9`

**问题**：`rlib` 是 workspace library crate 的默认 crate-type，显式声明无任何效果。

**方案**：从上述 6 个 Cargo.toml 中移除 `crate-type = ["rlib"]` 行（以及包裹它的 `[lib]` 表，如果 `[lib]` 中无其他项）。

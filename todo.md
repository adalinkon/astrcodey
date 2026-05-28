# 代码冗余清理

## ~~[中] `AgentSessionStatusDto` 与 core `AgentSessionStatus` 序列化完全一致~~ ✅ 已清理

将 `agent_session_link.rs` 中的 enum 定义替换为 `pub use astrcode_core::storage::AgentSessionStatus as AgentSessionStatusDto;`，删除冗余定义和 From impl。

## ~~[中] `KeybindingInfoDto` 与 core `Keybinding` 序列化完全一致~~ ✅ 已清理

- `protocol/events.rs`：删除 `KeybindingInfoDto` 结构体，`ExtensionCommandList` 改用 core 的 `Keybinding`
- `server/handler/router.rs`：移除逐字段手动转换，直接传递 `collect_keybindings()` 结果
- `cli/tui/app/handle_event.rs`：参数类型改为 core 的 `Keybinding`

## ~~[低] 6 个 extension crate 冗余 `crate-type = ["rlib"]`~~ ✅ 已清理

从 6 个 Cargo.toml 中移除了无意义的 `[lib]` 段。

# Session Runtime 优化计划

## 目标

提升 `astrcode-session` 的可读性、安全边界与模块边界，不改变对外 pub API（`Session`、`SessionError`、`TurnError` 等仍从 crate 根导出）。

## P0 — 已完成

| 项 | 做法 |
|---|---|
| CompactionCoordinator | 新增 `compaction_coordinator.rs`，`prepare_context_messages` 统一 auto / reactive compact |
| LifecycleContext 统一 | `SharedTurnContext::from_read_model` + `emit_lifecycle_for_read_model` 复用 |
| StepEnd 显式化 | `on_step_end_best_effort()` 替代 silent `let _ = ...` |

## P1 — 已完成

| 项 | 做法 |
|---|---|
| 拆分 session.rs | `src/session/{mod,events,prompt,turn_entry,children,compact}.rs` |
| TurnError 结构化 | `SessionReadFailed`、`StreamEndedUnexpectedly`、`ProviderBlocked` 等 |
| AgentSignal 简化 | `TurnEventTx` 类型别名；`AgentSignal` 保留为兼容别名 |
| dispatch 改进 | `dispatch_turn_event` durable 失败时 `tracing::warn` |

## P2 — 已完成

| 项 | 做法 |
|---|---|
| SessionCreateParams | 新增 struct；`create_with_id` 委托 `create_with_params` |
| 命名/注释 | TurnRunner 模块注释统一；`drain_completed` signal 语义文档化 |
| ToolRuntimeCapabilities::for_turn | TurnRunner 构造时使用工厂 |

## 已拒绝 / 已移除

| 项 | 说明 |
|---|---|
| `max_steps` / step limit | 不在 `AgentSettings`（effective）中暴露；`TurnRunner` 不强制 step 上限；`TurnError::StepLimitExceeded` 已删除 |

## 测试

- `tool_pipeline`: parallel / sequential / blocked 调度顺序

## 验证

```bash
cargo fmt
cargo test -p astrcode-session -p astrcode-core -p astrcode-server
cargo clippy -p astrcode-session -p astrcode-core -p astrcode-server --all-targets -- -D warnings
```

## 暂缓

- `TurnRunner::from_session` 别名：构造依赖过多，收益低
- 服务端 `TurnError` 细粒度客户端错误码：当前经 `HandlerError::Turn` 透传 `Display`，除 `TurnAlreadyRunning` 外未单独映射
- Mid-turn inbox 合并（P3）
- `visible_tools` 缓存

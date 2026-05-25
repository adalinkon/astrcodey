# AstrCode 扩展系统

> **本文档是 AstrCode 扩展机制的唯一规范说明。**  
> 内容以当前代码为准（`astrcode-core`、`astrcode-extension-sdk`、`astrcode-extensions`、`astrcode-server`）。  
> 历史文档 `plugin-system.md`（AstrBot s5r / IPC 起源）**不是**实现目标，仅作背景参考。

---

## 目录

1. [概览](#1-概览)
2. [代码地图](#2-代码地图)
3. [内置扩展（进程内）](#3-内置扩展进程内)
4. [磁盘 WASM 扩展](#4-磁盘-wasm-扩展)
5. [s6r 线缆协议](#5-s6r-线缆协议)
6. [宿主 import：`host_log` / `host_emit` / `host_invoke`](#6-宿主-importhost_log--host_emit--host_invoke)
7. [运行时模型](#7-运行时模型)
8. [Guest 作者指南](#8-guest-作者指南)
9. [新增宿主能力 checklist](#9-新增宿主能力-checklist)
10. [边界与测试](#10-边界与测试)

---

## 1. 概览

AstrCode 的 **Extension（扩展）** 是主要可扩展机制：Agent 工具、斜杠命令、生命周期钩子、Prompt 片段、自定义 extension event 均通过 `Extension` trait 注册，由 `ExtensionRunner` 分发。

| 层级 | 实现 | 信任模型 |
|------|------|----------|
| **内置** | `astrcode-bundled-extensions` + 各 `astrcode-extension-*` | 进程内可信代码；通过 `ExtensionCtx::host_services()` 访问 `ExtensionHostServices` |
| **磁盘 WASM** | `~/.astrcode/extensions/`、`<project>/.astrcode/extensions/` | wasmtime 沙箱 + s6r 协议 + 能力白名单 |
| **外部工具** | `astrcode-extension-mcp` | MCP 子进程/HTTP；**不**实现 `Extension` trait |

**不做的事：**

- 磁盘路径**仅支持 `.wasm`**，无 native dlopen / FFI 加载。
- 不把 s5r IPC / `platform.*` / STDIO 插件当作 AstrCode coding agent 的实现目标（MCP 已覆盖「任意语言外部工具」）。
- bundled 扩展**不走 WASM**（`Arc<dyn LlmProvider>` 无法跨 WASM 边界，且 core 依赖在 wasm32 下无法完整编译）。

---

## 2. 代码地图

| Crate / 模块 | 职责 |
|--------------|------|
| `astrcode-core::extension` | `Extension` trait、`ExtensionCapability`、`ExtensionEventSink`、`Registrar`、各 Hook 上下文 |
| `astrcode-extension-sdk` | 扩展作者公共 API；`s6r` 线缆类型（`Manifest`、`CallRequest`、`CallResponse`） |
| `astrcode-extensions::loader` | 磁盘发现、`extension.json` 解析、WASM 加载 |
| `astrcode-extensions::runner` | `ExtensionRunner`：注册、hook 分发、能力门控、event sink 绑定 |
| `astrcode-extensions::wasm_ext` | `WasmExtension`：s6r 桥接、专用 guest 线程 |
| `astrcode-extensions::wasm_api` | wasmtime `HostState`、linker、内存读写、host import |
| `astrcode-extensions::host_invoke` | `host_invoke` 权限表 + `build_small_llm_invoker` |
| `astrcode-extensions::host_emit` | `host_emit` 权限与声明校验 |
| `astrcode-server::bootstrap` | 启动时注入 `build_small_llm_invoker` 到 `ExtensionLoadContext` |

**参考实现：**

- 进程内扩展：`crates/astrcode-extension-memory`
- WASM guest 示例：`crates/astrcode-extensions/tests/s6r-guest/`

---

## 3. 内置扩展（进程内）

### 3.1 Extension trait

每个扩展实现 `Extension`：

- `id()` — 唯一标识
- `capabilities()` — 声明需要的 `ExtensionCapability`（默认 `[]`）
- `register(&mut Registrar)` — 注册 tools、commands、hooks、extension events
- `start(ExtensionCtx)` / `stop(StopReason)` / `health()` / `on_config_changed()`

`ExtensionCtx` 在 `start()` 时提供：

- `startup_working_dir`
- `event_sink`（启动阶段 emit 自定义 event）
- `host_services`（`ExtensionHostServices`：可选 `session_read`、`small_llm`）

### 3.2 ExtensionCapability

定义于 `astrcode-core::extension::ExtensionCapability`（serde `snake_case`）：

| 枚举 | 线字符串 | 含义 |
|------|---------|------|
| `SessionState` | `session_state` | 访问 session 命名空间状态目录 |
| `SessionControl` | `session_control` | 子 session / turn 控制 |
| `SmallModel` | `small_model` | 调用宿主小模型（WASM 侧对应 `host_invoke("small_llm.chat")`） |
| `SessionHistory` | `session_history` | 只读历史 session 投影 |
| `EmitEvents` | `emit_events` | 发射已声明的 extension event |
| `WorkspaceRead` | `workspace_read` | 读取工作区（预留） |
| `ProcessSpawn` | `process_spawn` | 子进程（预留） |
| `NetworkClient` | `network_client` | 网络客户端（预留） |

`ExtensionRunner` 按声明裁剪注入到工具/钩子上下文的敏感字段（例如未声明 `session_state` 时清除 `session_store_dir`）。

### 3.3 Hook 模式

| 模式 | 行为 |
|------|------|
| `Blocking` | 同步；可 block / modify |
| `NonBlocking` | 异步 fire-and-forget |
| `Advisory` | 执行但结果仅供参考 |

支持的生命周期事件见 [§5.3 hooks 事件名](#53-manifestextension_manifest-返回)。

### 3.4 Extension event

内置扩展通过 `Registrar::extension_event("type.name")` 声明可 emit 的类型；运行时 `BoundExtensionEventSink` 校验 `event_type`、`schema_version`、payload 大小后写入会话事件流（`EventPayload::ExtensionEvent`）。

---

## 4. 磁盘 WASM 扩展

### 4.1 目录布局

```
~/.astrcode/extensions/<name>/
  extension.json      # 发现入口
  my_ext.wasm         # library 指向的文件

<project>/.astrcode/extensions/<name>/
  extension.json
  ...
```

项目级扩展在加载顺序中优先于全局扩展。

### 4.2 extension.json

**Loader 实际只读取 `library`（必填，相对路径指向 `.wasm`）。**

```json
{
  "library": "my_extension.wasm"
}
```

`extension.json` 中的 `id`、`name`、`capabilities` 等字段可被 serde 解析（供 UI/诊断），**不参与加载**——真实 `id`、能力、工具、hook 均由 guest 的 `extension_manifest()` 返回。

`ExtensionManifest` 类型（`astrcode-core`）保留完整字段以兼容旧清单，但 s6r loader 不依赖它们。

### 4.3 加载流程

```
extension.json.library
  → WasmExtension::load(path, fuel, memory_bytes, invoker)
  → instantiate + extension_manifest()     # 此阶段 invoker 未绑定
  → 解析 Manifest，校验 s6r == "1"
  → HostState::finish_manifest(capabilities, invoker)
  → 注册 tools / commands / hooks / extension_events 到 WasmExtension
  → ExtensionRunner::register
```

Server bootstrap（`astrcode-server/src/bootstrap/mod.rs`）构建 `ExtensionLoadContext`：

- `wasm_limits` — 来自配置 `wasmFuel` / `wasmMemoryMb`
- `invoker` — `Some(build_small_llm_invoker(small_llm))` 或 `None`

---

## 5. s6r 线缆协议

协议版本常量：`astrcode-extension-sdk::s6r::S6R_VERSION = "1"`。

设计原则：**两个 guest 导出、JSON 消息、声明式 manifest**，无 `extension_init` 副作用注册、无 effect 魔术整数。

### 5.1 Guest 导出

| 函数 | 签名 | 说明 |
|------|------|------|
| `memory` | export | 线性内存 |
| `alloc` | `(len: i32) -> i32` | 分配；失败返回 0 |
| `dealloc` | `(ptr: i32, len: i32)` | 释放 |
| `extension_manifest` | `() -> i64` | 返回 packed manifest JSON |
| `extension_call` | `(req_ptr, req_len) -> i64` | 返回 packed CallResponse JSON |

**packed i64** = `(ptr as u64) << 32 | (len as u64)`。宿主/guest 读取 JSON 后必须对 guest 分配的指针调用 `dealloc`。

**内存所有权：**

- 宿主通过 guest `alloc` 写入请求 → 调用结束后宿主 `dealloc`
- guest 在 `extension_manifest` / `extension_call` 内分配响应 → 宿主读取后 `dealloc`

### 5.2 Host import

注册于 `wasm_api::create_linker`（模块 `env`）：

| import | 签名 | 说明 |
|--------|------|------|
| `host_log` | `(level, msg_ptr, msg_len)` | level: 0=trace … 4=error |
| `host_emit` | `(event_ptr, event_len) -> i64` | 见 [§6.2](#62-host_emit) |
| `host_invoke` | `(cap_ptr, cap_len, input_ptr, input_len) -> i64` | 见 [§6.3](#63-host_invoke) |
| WASI preview1 | — | `wasm32-wasip1` guest 需要 |

`host_emit` / `host_invoke` 成功时返回 packed ResultMsg JSON；失败返回 `0`。

### 5.3 Manifest（`extension_manifest` 返回）

```json
{
  "s6r": "1",
  "id": "my-extension",
  "version": "0.1.0",
  "description": "可选",
  "capabilities": ["small_model", "emit_events"],
  "tools": [
    {
      "name": "grep_files",
      "description": "搜索文件",
      "parameters": { "type": "object", "properties": { "pattern": { "type": "string" } } },
      "mode": "parallel"
    }
  ],
  "commands": [{ "name": "hello", "description": "打招呼" }],
  "hooks": [
    { "on": "pre_tool_use", "mode": "blocking" },
    { "on": "turn_end", "mode": "non_blocking" }
  ],
  "extension_events": [
    {
      "event_type": "my_ext.done",
      "schema_version": 1,
      "durable": true,
      "max_payload_bytes": 65536
    }
  ]
}
```

`hooks[].on` 映射（实现：`s6r::event_from_name`）：

`session_start` · `session_resume` · `session_shutdown` · `turn_start` · `turn_end` · `turn_aborted` · `step_start` · `step_end` · `pre_tool_use` · `post_tool_use` · `post_tool_use_failure` · `before_provider_request` · `after_provider_response` · `user_prompt_submit` · `prompt_build` · `pre_compact` · `post_compact` · `post_recap`

`hooks[].mode`：`blocking` | `non_blocking` | `advisory`

未知 hook 名会被 warn 并忽略。

### 5.4 CallRequest（宿主 → guest）

serde 内部 tag 字段 `call`：

```json
{ "call": "tool",    "id": "req-1", "name": "grep_files", "input": { ... } }
{ "call": "hook",    "id": "req-2", "on": "pre_tool_use", "input": { ... } }
{ "call": "command", "id": "req-3", "name": "hello",       "input": { ... } }
```

`input` 由宿主序列化对应上下文（`ToolExecutionContext`、各 `*Context` hook 结构等）。

### 5.5 CallResponse（guest → 宿主）

```json
{ "id": "req-1", "ok": true,  "effect": "ok" }
{ "id": "req-2", "ok": true,  "effect": "block", "data": { "reason": "..." } }
{ "id": "req-3", "ok": false, "error": "pattern is required" }
```

**Effect 字符串**（`wasm_ext` 解析）：

| effect | 用途 | data |
|--------|------|------|
| `ok` | 默认成功 | 工具：`data.content` 文本 |
| `block` | blocking hook 阻止 | `{ "reason" }` |
| `modified_input` | 修改工具入参 | `{ "tool_input" }` |
| `tool_outcome` | 自定义工具结果 | `{ "outcome" }` |
| `prompt_contributions` | PromptBuild | `PromptContributions` |
| `compact_contributions` | Compact | `CompactContributions` |
| `replace_messages` / `append_messages` | Provider hook | `{ "messages" }` |

### 5.6 Continuations

guest 可在成功响应中附带 `continuations`（`CallContinuation` 数组，无 `id`）：

```json
{
  "id": "req-turn-end",
  "ok": true,
  "effect": "ok",
  "continuations": [
    { "call": "hook", "on": "pipeline_step", "input": { "step": 1 } },
    { "call": "tool", "name": "pipeline_status", "input": {} }
  ]
}
```

宿主在当前 `extension_call` 返回后**顺序**调度 follow-up，链深度上限 **16**（`MAX_CONTINUATION_DEPTH`）。适用于 NonBlocking hook 多段管线（参考 `s6r-guest` 的 `turn_end` + `pipeline_status`）。

---

## 6. 宿主 import：`host_log` / `host_emit` / `host_invoke`

### 6.1 ResultMsg 格式

`host_emit` 与 `host_invoke` 写入 guest 内存的 JSON：

```json
{ "ok": true,  "output": { ... } }   // host_invoke 成功
{ "ok": true }                       // host_emit 成功
{ "ok": false, "error": "..." }
```

guest 读取后必须 `dealloc(resp_ptr, resp_len)`。

### 6.2 host_emit

**权限：** manifest 须声明 `emit_events`（`host_emit::authorize_emit`）。

**输入（EmitEventMsg）：**

```json
{
  "event_type": "my_ext.done",
  "schema_version": 1,
  "payload": { "key": "value" }
}
```

**校验：** `event_type` 必须在 manifest `extension_events` 中声明；`schema_version` 不得超过声明值；payload 序列化字节数 ≤ `max_payload_bytes`（默认 65536）。

**投递：** 写入 `EventPayload::ExtensionEvent` 到会话事件通道。

**当前绑定范围：** 工具执行路径——`ToolExecutionContext.event_tx` 存在且扩展声明 `emit_events` 时，在 guest 调用前注入 `HostEmitSession`。Hook / lifecycle 路径尚未桥接 `extension_event_sink`（调用会返回 `"emit session not configured"`）。

实现：`host_emit.rs` + `wasm_api::host_emit`。

### 6.3 host_invoke

**ABI：**

```
host_invoke(cap_ptr, cap_len, input_ptr, input_len) -> i64
```

**权限：** `host_invoke::authorize` 对照 `HostState.declared_capabilities`（与 `ExtensionRunner` 的 per-extension allows 同源）。

| 能力名 | 须声明 capability | 后端 |
|--------|------------------|------|
| `small_llm.chat` | `small_model` | `build_small_llm_invoker` |

**`small_llm.chat` 输入：**

```json
{
  "messages": [{ "role": "user", "content": "hello" }],
  "max_tokens": 512
}
```

**output：**

```json
{ "content": "...", "model": "small_llm" }
```

**实现要点：**

- `build_small_llm_invoker` 在 server bootstrap 注入；manifest 阶段 `invoker = None`，防止未授权调用。
- LLM 在 Tokio runtime worker 上 `spawn` 异步执行；同步闭包通过 `sync_channel` 等待结果（**不对 runtime `block_on`**）。
- 超时：**30s**（`HOST_INVOKE_TIMEOUT`）。
- `invoker = None` 或 guest `alloc` 失败 → 返回 `0`。

---

## 7. 运行时模型

### 7.1 WASM 并发

- 每个 `WasmExtension` 在**专用 OS 线程**（`wasm-{extension_id}`）上串行执行 wasmtime 调用。
- Async 侧（`ExtensionRunner`、工具 pipeline）通过 `oneshot` 等待 guest 结果，**不占用** Tokio blocking 线程池。
- `wasmtime::Store` 为 `!Send`，由 `parking_lot::Mutex<WasmInner>` 保护。

### 7.2 资源限制

来自配置（默认见 `configuration_cn.md`）：

- `fuel` — 每次 guest 调用独立重置（默认 10_000_000 量级，可配置）
- `memory_bytes` — 线性内存增长上限（默认 64–128 MB 量级）

fuel 耗尽 → `ExtensionError::Timeout`。

### 7.3 能力门控（Runner）

`HandlerTool::execute` 示例（`runner.rs`）：

- 无 `session_state` → 清除 `session_store_dir`
- 无 `session_control` → 清除 `session_ops` / `small_model_id`
- 有 `emit_events` + `event_tx` → 绑定 `BoundExtensionEventSink`

WASM 扩展的 manifest capabilities 在 load 时写入 `WasmExtension`，注册时同步到 runner index。

---

## 8. Guest 作者指南

### 8.1 编译目标

```bash
rustup target add wasm32-wasip1
cargo build --target wasm32-wasip1 --release
```

### 8.2 最小目录

```
my-ext/
  extension.json    # { "library": "my_ext.wasm" }
  my_ext.wasm
```

### 8.3 必读示例

完整 Rust guest：`crates/astrcode-extensions/tests/s6r-guest/`  
涵盖：`extension_manifest`、`extension_call`、`host_invoke("small_llm.chat")`、continuations、pre_tool_use blocking。

E2E 测试：`crates/astrcode-extensions/tests/s6r_e2e_test.rs`（需先编译 guest WASM）。

### 8.4 SDK 类型

直接使用 `astrcode-extension-sdk::s6r::{Manifest, CallRequest, CallResponse, CallContinuation, S6R_VERSION}`。

guest 需自行实现 `alloc` / `dealloc` / `extension_manifest` / `extension_call` 导出（暂无 proc macro）。

---

## 9. 新增宿主能力 checklist

扩展 WASM 可调用的宿主能力时：

1. 若需新声明 → 在 `ExtensionCapability` 增加枚举项
2. 在 `host_invoke::required_capability` 表登记能力名 → capability 映射
3. 实现后端（参考 `build_small_llm_invoker` 或新增 builder）
4. 在 `wasm_api::create_linker` 确认 import 已注册（若走新 import 而非 `host_invoke`）
5. 更新本文档 Guest / 能力表
6. 测试：
   - `host_invoke::authorize` / `host_emit::authorize_emit` 单元测试
   - `wasm_ext_integration_test`（WAT 或合成模块）
   - 必要时扩展 `s6r-guest` + `s6r_e2e_test`

---

## 10. 边界与测试

### 10.1 明确不做

| 项 | 原因 |
|----|------|
| s5r IPC / STDIO 插件平台 | 与 coding agent 产品面不符；MCP 覆盖外部工具 |
| 磁盘 native dlopen | 已移除；仅 WASM |
| bundled 扩展 WASM 化 | 可信代码 in-process 更简单 |
| guest 内 `Handle::block_on` Tokio | 死锁风险；用 channel 等待 async 任务 |
| 无声明的 host 能力 | 必须 manifest + authorize |

### 10.2 测试覆盖

| 测试 | 覆盖 |
|------|------|
| `loader_integration_test` | 目录发现、manifest 解析、s6r 事件名 roundtrip |
| `wasm_ext_integration_test` | WAT 合成模块：manifest、tool/hook、host_emit、extension_events |
| `s6r_e2e_test` | 真实 `s6r-guest` WASM：tools、hooks、host_invoke、continuations |
| `host_invoke` / `host_emit` 单元测试 | authorize、emit 校验 |

运行：

```bash
# 先编译 guest（s6r e2e 需要）
cargo build -p s6r-guest --target wasm32-wasip1 --release

cargo test -p astrcode-extensions --all-targets
```

### 10.3 已知限制

- `host_emit` 仅在工具执行且 `event_tx` 可用时生效；hook 内 emit 待 runner 桥接。
- 每个 WASM 扩展一个 OS 线程；扩展数量大时需后续改为共享线程池。
- `extension.json` 的 `capabilities` 字段不参与加载，以 WASM manifest 为准。

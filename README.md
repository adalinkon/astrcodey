# AstrCode

**BE PI OR BETTER THAN PI**
Inspired by Claude Code, Codex, OpenCode, and Pi — but built as a Rust-native

cli：
<img width="1210" height="924" alt="image" src="https://github.com/user-attachments/assets/55259723-9bd7-4a1a-a74e-1e799ece2eed" />

app：
web：
<img width="1401" height="995" alt="image" src="https://github.com/user-attachments/assets/4e59f8fe-2344-4e78-ab36-c1fb19c549fc" />


A Rust-built AI coding agent platform.

AstrCode is a full-stack AI coding assistant built from scratch in ~55k lines of Rust across 21 crates, plus a React + TypeScript web frontend (~4.8k lines). It features an agent loop with tool execution, a streaming SSE-based multi-provider LLM layer (Anthropic, OpenAI, Google GenAI), a extension/hook extension system (with native extension loading via FFI and WASM extension support), context window management with auto-compaction, an eval framework for automated benchmarking, and multiple interfaces: a terminal UI, a web frontend, a Tauri desktop app, an HTTP/SSE API, and an ACP (Agent Client Protocol) adapter.

## Quick Start

```bash
# Build backend
cargo build

# Interactive terminal UI
cargo run -- tui

# Headless single-shot execution
cargo run -- exec "explain the agent loop architecture"

# HTTP/SSE server
cargo run -- server

# Web frontend (dev server)
cd frontend && npm install && npm run dev

# Tauri desktop app (dev mode)
cd frontend && npm install && npm run tauri:dev

# Eval framework (requires dev-mode feature)
cargo run --features dev-mode -- eval
```

## Architecture

```
          ┌──────────┐  ┌──────────────────────┐  ┌───────────┐
          │   TUI    │  │ Web / Tauri Frontend  │  │ ACP Client│
          │ (ratatui)│  │ React 19 + TypeScript │  │  (stdio)  │
          └────┬─────┘  └────────┬─────────────┘  └─────┬─────┘
               │                  │ SSE / JSON-RPC       │ ACP JSON-RPC
               │    stdio         │                      │ over stdio
               └────────┬────────┘──────────────────────┘
                   ┌─────┴──────┐
                   │astrcode-cli│  TUI / exec / server launcher
                   └─────┬──────┘
                         │
                   ┌─────┴──────┐
                   │astrcode-   │  Session management, JSON-RPC + HTTP handler
                   │server      │  ACP adapter, transport, concurrency control
                   └─────┬──────┘
                         │
                   ┌─────┴───────┐
                   │astrcode-    │  Agent loop core: turn runner, tool pipeline
                   │session      │  LLM stream, context compaction orchestration
                   └─────┬───────┘
             ┌───────────┼───────────┐
             │           │           │
    ┌────────┴───┐ ┌─────┴─────┐ ┌───┴──────────┐
    │ astrcode-ai│ │astrcode-  │ │ astrcode-    │
    │            │ │extensions │ │ tools        │
    │ Anthropic  │ │Hook system│ │File/shell/   │
    │ OpenAI     │ │Native FFI │ │task tools    │
    │ Google     │ │WASM ext   │ │              │
    │ SSE+retry  │ │           │ │              │
    └────────┬───┘ └─────┬─────┘ └──────────────┘
             │           │
   ┌─────────┴──┐  ┌────┴────────────┐
   │astrcode-   │  │ Extension crates │
   │ context    │  │ ├ mcp            │
   │ Token budget│  │ ├ skill         │
   │ Auto-compact│  │ ├ todo-tool     │
   └────────────┘  │ ├ mode          │
                   │ └ agent-tools   │
                   └─────────────────┘
        ┌─────────────────────────────┐
        │        Shared layer         │
        │ core · protocol · storage   │
        │ support · log               │
        └─────────────────────────────┘
```

## Crates

| Crate | Lines | Description |
|---|---|---|
| `astrcode-server` | 9.5k | Session management, JSON-RPC/HTTP/ACP handlers, transport, concurrency control |
| `astrcode-cli` | 8.0k | Terminal UI (ratatui), headless exec, server launcher |
| `astrcode-session` | 5.2k | Agent loop core: turn runner, tool pipeline, LLM stream consumption, compact orchestration |
| `astrcode-core` | 4.9k | Shared types, traits, config system, error types, prompt composition, extension contracts |
| `astrcode-tools` | 4.6k | Built-in tools: read, write, edit, patch, find, grep, shell, terminal, task |
| `astrcode-storage` | 3.7k | JSONL event log, session snapshots, config persistence, file locking |
| `astrcode-ai` | 3.6k | Multi-provider LLM layer (Anthropic, OpenAI, Google GenAI), SSE streaming, retry |
| `astrcode-context` | 3.5k | Token estimation, context window budgeting, auto-compact, prompt engine |
| `astrcode-extensions` | 2.8k | Extension lifecycle, hook dispatch, native extension loading (FFI), WASM extension runtime |
| `astrcode-extension-mcp` | 1.9k | MCP protocol client via stdio, tool discovery |
| `astrcode-protocol` | 1.2k | JSON-RPC 2.0 wire types, commands, events, HTTP DTOs |
| `astrcode-extension-mode` | 1.2k | Agent running mode switching (Code / Plan), plan artifact, exit gate, keybinding & status item registration |
| `astrcode-eval` | 1.1k | Eval framework — HTTP server control, event log metrics, structured reporting |
| `astrcode-extension-skill` | 949 | Slash-command skill discovery and dispatch |
| `astrcode-extension-todo-tool` | 733 | Progress tracking todo list tool |
| `astrcode-extension-agent-tools` | 704 | Sub-agent delegation, agent discovery (Claude Code compatible format) |
| `astrcode-support` | 682 | Path resolution, shell detection, text processing |
| `astrcode-client` | 521 | Typed JSON-RPC client, transport, stream subscription |
| `astrcode-log` | 353 | File rotation, stderr output, env-filter logging |
| `astrcode-bundled-extensions` | 39 | Composition root for optional extension crates |

**Total: ~55k lines across 20 Rust crates + Tauri shell, 203 source files.**

### Frontend & Desktop App

| Component | Lines | Description |
|---|---|---|
| `frontend/` (React + TS) | ~4.8k | Web frontend — chat view, sidebar, session management, SSE streaming |
| `src-tauri/` (Tauri v2) | ~670 | Desktop app shell — sidecar management, single-instance coordination, native dialogs |

The web frontend (`frontend/`) is a React 19 + TypeScript + Tailwind CSS v4 + Vite single-page application. It connects to the `astrcode-server` backend via SSE for real-time streaming and JSON-RPC for commands. The frontend supports running standalone in the browser (`npm run dev`) or packaged as a Tauri desktop app (`npm run tauri:dev`).

The Tauri desktop app (`src-tauri/`) wraps the web frontend in a native window and manages the `astrcode-server` as a sidecar process — automatically launching it on startup, discovering a free port, and bridging the connection. It also provides single-instance coordination (file-lock + TCP activation) and native file dialogs via `tauri-extension-dialog`.

## Key Design Decisions

### Agent Loop

The agent loop (`astrcode-session`) follows a phased pipeline pattern:

1. **Prepare context** — token budget check, auto-compact if needed
2. **Build provider request** — hook dispatch, message assembly, MCP tool discovery
3. **Stream LLM response** — SSE parsing, UTF-8 safe decoding, event accumulation
4. **Execute tools** — parallel batch execution with pre/post hooks, result persistence
5. **Loop or return** — tool calls loop back; text-only responses terminate

The agent supports running mode switching (Code / Plan). Plan mode restricts tools to read-only and plan management, enforces an exit gate (self-review checklist + required heading validation), and persists the plan artifact to `<session>/plan/plan.md`. Mode instructions are injected via `BeforeProviderRequest`, preserving the system prompt KV cache.

The `ToolPipeline` struct owns tool preprocessing, parallel scheduling, and result persistence. The `SharedTurnContext` struct carries session-level identifiers. `consume_llm_stream` returns a `StreamOutcome` enum (`Complete` | `ToolCalls`) that makes the loop body read as a linear sequence of named phases.

### LLM Provider Layer

`astrcode-ai` supports multiple providers — Anthropic (native Messages API), OpenAI-compatible (Chat Completions + Responses API), and Google GenAI. Key components:

- **`Utf8StreamDecoder`** — handles multi-byte UTF-8 boundaries and bad-byte recovery across TCP chunks
- **`SseLineReader`** — generic SSE line buffering (reusable across all providers)
- **`RetryPolicy`** — exponential backoff with jitter for 429/5xx errors

### Context Window Management

When conversation history approaches 83.5% of the model's context limit, `astrcode-context` triggers automatic compaction:

1. LLM-backed compaction (model generates a structured 9-section summary) runs by default for both auto and manual compact
2. On LLM failure (network error, parse error, timeout), the system falls back to deterministic rule-based summarization
3. Compact transcripts are persisted as snapshots for debugging
4. Post-compact context restoration re-reads recent files and preserves agent/skill/tool state

### Tool Execution

Tools run in parallel batches (up to 5 concurrent). The pipeline:

1. **Prepare** — parse JSON args (with repair for malformed LLM output), check visibility, dispatch `PreToolUse` hooks
2. **Execute** — parallel batch via `JoinSet`, sequential tools flush the batch first
3. **Commit** — dispatch `PostToolUse` hooks, persist large results, enforce message budget, emit events

Large tool results are automatically persisted to disk and replaced with preview summaries to stay within the message character budget.

### Extension System

The extension system (`astrcode-extensions`) is a core architectural pillar, not an afterthought:

- **Extension trait** — each extension declares hook subscriptions, contributes tools and slash commands, handles lifecycle events
- **Hook modes** — `Blocking` (can modify input/output), `NonBlocking` (fire-and-forget), `Advisory` (observe-only)
- **Keybinding registration** — extensions register keyboard shortcuts (e.g. `Shift+Tab` for mode toggle) via `Registrar::keybinding()`
- **Status bar items** — extensions contribute status bar entries (e.g. current mode indicator) with runtime updates via `StatusItemUpdate` notifications
- **Native extension loading** — disk-loaded `.dll`/`.so` extensions via `libloading` + FFI, supporting global (`~/.astrcode/extensions/`) and project-level (`.astrcode/extensions/`) directories
- **WASM extension runtime** — wasmtime-based sandboxed extension execution with a host-guest protocol for tool registration and event handling
- **Extension runtime** — session spawning with depth limits, tool registration queue, priority-based dispatch

### ACP Adapter

The ACP adapter (`astrcode-server::acp`) bridges the standard Agent Client Protocol to astrcode's internal command/broadcast architecture:

- Stdio JSON-RPC server implementing Initialize / NewSession / Prompt / Cancel
- Real-time event streaming via broadcast channel to ACP `SessionNotification`
- Deterministic event flushing with completion oneshot for turn lifecycle
- Designed for IDE extensions and editor integrations

## Running Modes

| Mode | Command | Description |
|---|---|---|
| **TUI** | `cargo run -- tui` | Interactive terminal UI with message history, tool display, slash commands |
| **Exec** | `cargo run -- exec "prompt"` | Headless single-shot execution, supports `--jsonl`|
| **Server** | `cargo run -- server [--addr 0.0.0.0:3847]` | HTTP/SSE server with JSON-RPC, session management, real-time event streaming |
| **ACP** | `cargo run -- acp` | ACP stdio adapter for IDE/editor integration |
| **Eval** | `cargo run --features dev-mode -- eval` | Run evaluation benchmarks (requires `dev-mode` feature) |
| **Web** | `cd frontend && npm run dev` | Browser-based chat interface connected to the server via SSE |
| **Desktop** | `cd frontend && npm run tauri:dev` | Tauri desktop app (auto-launches server as sidecar) |

### TUI Reference

**Keyboard Shortcuts:**

| Key | Action |
|---|---|
| `Enter` | Submit prompt / accept slash command selection |
| `Shift+Enter` / `Alt+Enter` | Insert newline |
| `Esc` | Close slash palette / stop streaming turn |
| `Tab` | Complete slash command selection |
| `Shift+Tab` | Trigger extension-registered keybinding |
| `Ctrl+A` / `Ctrl+E` | Move to start / end of line |
| `Ctrl+U` / `Ctrl+K` | Delete before / after cursor |
| `Ctrl+W` | Delete previous word |
| `Ctrl+C` | Quit (with confirmation) |

**Slash Commands:**

| Command | Description |
|---|---|
| `/new` | Create a fresh session |
| `/resume <id>` or `/r <id>` | Resume a previous session |
| `/sessions` or `/ls` | Open session picker |
| `/compact` | Compact the current session context |
| `/help` or `/?` | Show command help |
| `/quit` or `/q` | Exit astrcode |

extension extensions can register additional slash commands and keybindings at runtime.

## Distribution

Pre-built binaries are available for Linux, macOS, and Windows (x86_64 + aarch64) via GitHub Releases on every version tag. A weekly automated release pipeline publishes patch bumps every Monday.

## Acknowledgments

This project drew inspiration and design patterns from several open-source projects:

- **[Claude Code](https://docs.anthropic.com/en/docs/claude-code)** — tool execution pipeline, system prompt design
- **[OpenCode](https://github.com/anomalyco/opencode)** — the frontend-backend separation (HTTP/SSE + JSON-RPC) references OpenCode's architecture.
- **[Codex CLI](https://github.com/openai/codex)** — TUI layout and terminal UI design borrow from Codex's approach to rendering agent interactions in the terminal.

## License

AGPL-3.0

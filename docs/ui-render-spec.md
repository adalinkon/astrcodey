# 结构化 UI 渲染协议（ui_render / ui_summary）

工具可以通过 `ToolResult.metadata` 中的两个特殊键控制前端渲染，而不依赖特定前端形态。

| 键 | 类型 | 用途 |
|---|---|---|
| `"ui_render"` | `RenderSpec`（JSON 对象） | 描述结构化渲染布局，替代默认纯文本展示 |
| `"ui_summary"` | `string` | 一行折叠摘要，UI 用它覆盖工具调用摘要行 |

Web 前端从同一份 metadata 中提取渲染信息；其它客户端也可以复用该结构化协议自行决定展示方式。

---

## Rust 完整示例

```rust
use astrcode_core::render::{RenderSpec, RenderTone, RenderKeyValue,
    UI_RENDER_METADATA_KEY, UI_SUMMARY_METADATA_KEY};
use astrcode_core::tool::{ToolResult, tool_metadata};
use serde_json::json;

// 1. 构建 RenderSpec
let spec = RenderSpec::Box {
    title: Some("Plan created".into()),
    tone: RenderTone::Success,
    children: vec![RenderSpec::KeyValue {
        entries: vec![
            RenderKeyValue { key: "path".into(), value: "plan.md".into(), tone: RenderTone::Default },
            RenderKeyValue { key: "tasks".into(), value: "5 items".into(), tone: RenderTone::Accent },
        ],
        tone: RenderTone::Default,
    }],
};

// 2. 返回带 ui_render + ui_summary 的 ToolResult
Ok(ToolResult::text(
    "Plan artifact created at plan.md.".into(),
    false,
    tool_metadata([
        ("path", json!("plan.md")),
        (UI_RENDER_METADATA_KEY, serde_json::to_value(&spec).unwrap()),
        (UI_SUMMARY_METADATA_KEY, json!("Plan created: 5 tasks")),
    ]),
))
```

---

## JSON 示例

以下是 `ui_render` + `ui_summary` 在 `ToolResult.metadata` 中的实际 JSON 结构：

```json
{
  "ui_render": {
    "type": "box",
    "title": "Plan created",
    "tone": "success",
    "children": [
      {
        "type": "key_value",
        "entries": [
          { "key": "path", "value": "plan.md" },
          { "key": "tasks", "value": "5 items", "tone": "accent" }
        ]
      }
    ]
  },
  "ui_summary": "Plan created: 5 tasks"
}
```

---

## RenderSpec 节点类型

所有节点共用 `#[serde(tag = "type", rename_all = "snake_case")]`，即 JSON 中用 `"type"` 字段区分变体。

| `type` | 必填字段 | 可选字段 | 说明 |
|---|---|---|---|
| `text` | `text` | `tone` | 普通文本 |
| `markdown` | `text` | `tone` | Markdown 文本，客户端可按自身能力选择富文本或安全纯文本展示 |
| `box` | — | `title`, `tone`, `children` | 分组容器，`children` 为子节点列表 |
| `list` | — | `items`, `ordered`, `tone` | 列表，`ordered` 控制有序/无序 |
| `key_value` | — | `entries`, `tone` | 键值表，`entries` 为 `[{key, value, tone?}]` 数组 |
| `progress` | `label` | `status`, `value`, `tone` | 进度状态，`value` 范围 0.0–1.0 |
| `diff` | `text` | `tone` | Diff 文本 |
| `code` | `text` | `language`, `tone` | 代码块 |
| `image_ref` | `uri` | `alt`, `tone` | 图片引用 |
| `raw_ansi_limited` | `text` | `tone` | 受限 ANSI 文本，客户端可选择去除或裁剪控制序列 |

## RenderTone

所有节点都支持可选的 `tone` 字段，由具体前端主题或客户端皮肤映射到颜色和样式：

| tone | 语义 |
|---|---|
| `"default"` | 默认正文 |
| `"muted"` | 次要文本 |
| `"accent"` | 强调文本 |
| `"success"` | 成功状态 |
| `"warning"` | 警告状态 |
| `"error"` | 错误状态 |

---

## 前端消费（TypeScript）

前端通过 `extractRenderSpec` / `extractRenderSummary` 提取渲染信息：

```typescript
import { extractRenderSpec, extractRenderSummary } from '../types/render-spec'

const spec = extractRenderSpec(toolResult.metadata)   // RenderSpec | undefined
const summary = extractRenderSummary(toolResult.metadata) // string | undefined
```

当 `ui_render` 不存在或格式不合法时，前端应回退到渲染 `ToolResult.content` 纯文本。

---

## 组合示例

### 进度 + 键值表

```json
{
  "type": "box",
  "title": "Build status",
  "children": [
    {
      "type": "progress",
      "label": "Compiling",
      "status": "running",
      "value": 0.65,
      "tone": "accent"
    },
    {
      "type": "key_value",
      "entries": [
        { "key": "crate", "value": "astrcode-session" },
        { "key": "warnings", "value": "0", "tone": "success" }
      ]
    }
  ]
}
```

### 有序列表 + 代码块

```json
{
  "type": "list",
  "ordered": true,
  "items": [
    {
      "type": "code",
      "language": "rust",
      "text": "fn main() { println!(\"hello\"); }"
    },
    {
      "type": "text",
      "text": "Step 2: run `cargo build`",
      "tone": "muted"
    }
  ]
}
```

---

## 纯文本回退

每个 `RenderSpec` 都实现了 `plain_text_fallback()`，在旧渲染路径或错误恢复时使用：

```rust
let text = spec.plain_text_fallback();
```

前端侧对应 `renderSpecToPlainText(spec)`。

---

## 参考实现

| 层 | 文件 |
|---|---|
| Rust 核心类型 | `crates/astrcode-core/src/render.rs` |
| 工具使用示例 | `crates/astrcode-extension-mode/src/tools.rs`（`upsert_session_plan`） |
| 前端消费 | `frontend/src/components/Chat/RenderSpecViewer.tsx` |
| 前端类型 + 提取 | `frontend/src/types/render-spec.ts` |

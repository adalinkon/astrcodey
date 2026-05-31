import type { ConversationBlock } from '../services/types'
import type { RenderSpec } from '../types/render-spec'

export type ToolCallBlockModel = Extract<ConversationBlock, { kind: 'toolCall' }>
export type JsonRecord = Record<string, unknown>

/** 宿主渲染上下文（来自 conversation block，UI 契约来自 metadata.toolUi）。 */
export interface ToolUiContext {
  block: ToolCallBlockModel
  sessionId: string | null
  args: JsonRecord
  meta: JsonRecord
  renderSpec?: RenderSpec
}

import type { JsonRecord } from './types'

export const TOOL_UI_METADATA_KEY = 'toolUi'
export const TOOL_UI_PHASE_METADATA_KEY = 'toolUiPhase'

export type ToolUiPhase = 'input' | 'approval' | 'result'

export type ToolApprovalUiWire =
  | { kind: 'builtin'; variant: string }
  | { kind: 'schema'; schema: JsonRecord; uiSchema?: JsonRecord }

export type ToolUiWire = {
  input?: { kind: string }
  approval?: ToolApprovalUiWire
  result?: { kind: 'builtin'; variant: string }
}

export function readToolUi(meta: JsonRecord): ToolUiWire | undefined {
  const raw = meta[TOOL_UI_METADATA_KEY]
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return undefined
  return raw as ToolUiWire
}

export function readToolUiPhase(meta: JsonRecord): ToolUiPhase | undefined {
  const raw = meta[TOOL_UI_PHASE_METADATA_KEY]
  if (raw === 'input' || raw === 'approval' || raw === 'result') return raw
  return undefined
}

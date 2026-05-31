import type {
  AgentSessionLink,
  ConversationBlock,
  ConversationControlState,
  ExtensionStateView,
  KeybindingInfo,
  Phase,
  SessionListItem,
  SlashCommandInfo,
} from '../services/types'

export type MessageDelivery = 'queued' | 'inject'

export interface PendingMessage {
  id: string
  text: string
  delivery: MessageDelivery
}

export interface AppState {
  serverPort: number | null
  connectionStatus: 'disconnected' | 'connecting' | 'connected' | 'error'
  connectionError: string | null

  sessions: SessionListItem[]
  activeSessionId: string | null
  activeSessionTitle: string | null
  workingDir: string | null

  blocks: ConversationBlock[]
  control: ConversationControlState | null
  cursor: string | null
  phase: Phase
  compactSubmitting: boolean

  streamAbortController: AbortController | null
  modelRefreshKey: number
  agentSessions: AgentSessionLink[]
  statusItems: Record<string, string>
  keybindings: KeybindingInfo[]
  slashCommands: SlashCommandInfo[]
  extensions: ExtensionStateView[]
  transientHint: string | null
  pendingMessages: PendingMessage[]
  composerDeliveryMode: MessageDelivery

  initServer: () => Promise<void>
  refreshSessions: () => Promise<void>
  createSession: (workingDir: string) => Promise<void>
  deleteSession: (sessionId: string) => Promise<void>
  deleteProject: (workingDir: string) => Promise<void>
  bumpModelRefreshKey: () => void
  switchSession: (sessionId: string) => Promise<void>
  refreshConversationSnapshot: () => Promise<void>
  refreshExtensionData: () => Promise<void>
  refreshCommands: () => Promise<void>
  submitPrompt: (text: string) => Promise<boolean>
  abortCurrentTurn: () => Promise<void>
  applyDelta: (delta: import('../services/types').ConversationDelta) => void
  clearTransientHint: () => void
  toggleComposerDeliveryMode: () => void
  togglePendingDelivery: (id: string) => Promise<void>
  removePendingMessage: (id: string) => void
  restorePendingMessage: (id: string) => string | null
  flushPendingQueued: () => Promise<void>
}

import { create } from 'zustand'
import * as api from '../services/api'
import { resolveHostBridge } from '../lib/hostBridge'
import type { ConversationDelta } from '../services/types'
import { applyDeltaToState } from './delta/applyDelta'
import {
  commandNoteBlock,
  isCompactCommand,
  phaseFromControl,
  withTimeout,
} from './delta/blockHelpers'
import { connectSse } from './stream'
import type { AppState } from './types'

function resetSessionView(): Partial<AppState> {
  return {
    activeSessionId: null,
    activeSessionTitle: null,
    blocks: [],
    control: null,
    cursor: null,
    phase: 'idle',
    compactSubmitting: false,
    workingDir: null,
    agentSessions: [],
    queuedMessages: [],
    slashCommands: [],
    keybindings: [],
    statusItems: {},
  }
}

export const useAppStore = create<AppState>((set, get) => ({
  serverPort: null,
  connectionStatus: 'disconnected',
  connectionError: null,
  sessions: [],
  activeSessionId: null,
  activeSessionTitle: null,
  workingDir: null,
  blocks: [],
  control: null,
  cursor: null,
  phase: 'idle',
  compactSubmitting: false,
  streamAbortController: null,
  modelRefreshKey: 0,
  agentSessions: [],
  statusItems: {},
  keybindings: [],
  slashCommands: [],
  extensions: [],
  transientHint: null,
  queuedMessages: [],

  initServer: async () => {
    set({ connectionStatus: 'connecting', connectionError: null })

    const bridge = await resolveHostBridge()

    if (bridge.isDesktopHost) {
      try {
        const { invoke } = await import('@tauri-apps/api/core')
        const result = await withTimeout(
          invoke<{ port: number; token?: string }>('start_server'),
          15_000,
          '启动 AstrCode 服务超时，请关闭残留 astrcode-http-server 进程后重试'
        )
        api.setServerPort(result.port, result.token)
        set({ serverPort: result.port })
      } catch (err) {
        set({
          connectionStatus: 'error',
          connectionError: err instanceof Error ? err.message : String(err),
        })
        return
      }
    } else {
      api.initBaseUrl()
      const envToken = (
        import.meta as unknown as { env: Record<string, string> }
      ).env?.VITE_AUTH_TOKEN
      if (envToken) {
        api.setAuthToken(envToken)
      }
    }

    set({ connectionStatus: 'connected' })
    await get().refreshSessions()
    void get().refreshExtensionData()
  },

  refreshSessions: async () => {
    try {
      const response = await api.listSessions()
      set({ sessions: response.sessions })
    } catch (err) {
      console.error('Failed to refresh sessions:', err)
    }
  },

  createSession: async (workingDir: string) => {
    const response = await api.createSession(workingDir)
    await get().refreshSessions()
    await get().switchSession(response.sessionId)
  },

  deleteSession: async (sessionId: string) => {
    try {
      await api.deleteSession(sessionId)
    } catch (err) {
      console.error('Failed to delete session:', err)
    }
    const state = get()
    if (state.activeSessionId === sessionId) {
      state.streamAbortController?.abort()
      set(resetSessionView())
    }
    await get().refreshSessions()
  },

  deleteProject: async (workingDir: string) => {
    try {
      await api.deleteProject(workingDir)
    } catch (err) {
      console.error('Failed to delete project:', err)
    }
    const state = get()
    const activeSession = state.sessions.find(
      (s) => s.sessionId === state.activeSessionId
    )
    if (activeSession && activeSession.workingDir === workingDir) {
      state.streamAbortController?.abort()
      set(resetSessionView())
    }
    await get().refreshSessions()
  },

  bumpModelRefreshKey: () => {
    set((s) => ({ modelRefreshKey: s.modelRefreshKey + 1 }))
  },

  switchSession: async (sessionId: string) => {
    const state = get()
    state.streamAbortController?.abort()

    set({
      activeSessionId: sessionId,
      blocks: [],
      control: null,
      cursor: null,
      phase: 'idle',
      compactSubmitting: false,
      agentSessions: [],
      transientHint: null,
      queuedMessages: [],
      slashCommands: [],
      keybindings: [],
      statusItems: {},
    })

    try {
      const snapshot = await api.getConversation(sessionId)
      const sessions = get().sessions
      const sessionItem = sessions.find((s) => s.sessionId === sessionId)

      set({
        blocks: snapshot.blocks,
        control: snapshot.control,
        cursor: snapshot.cursor.value,
        phase: phaseFromControl(snapshot.control),
        activeSessionTitle: snapshot.sessionTitle,
        workingDir: sessionItem?.workingDir ?? null,
        agentSessions: snapshot.agentSessions ?? [],
      })

      connectSse(sessionId, snapshot.cursor.value, 0, get, set)
      void get().refreshCommands()
    } catch (err) {
      console.error('Failed to switch session:', err)
    }
  },

  refreshConversationSnapshot: async () => {
    const { activeSessionId } = get()
    if (!activeSessionId) return

    try {
      const snapshot = await api.getConversation(activeSessionId)
      set({
        blocks: snapshot.blocks,
        control: snapshot.control,
        cursor: snapshot.cursor.value,
        phase: phaseFromControl(snapshot.control),
        activeSessionTitle: snapshot.sessionTitle,
        agentSessions: snapshot.agentSessions ?? [],
      })
    } catch (err) {
      console.error('Failed to refresh conversation snapshot:', err)
    }
  },

  refreshExtensionData: async () => {
    try {
      const extensions = await api.listExtensions()
      set({ extensions })
    } catch (err) {
      console.error('Failed to refresh extensions:', err)
    }
  },

  refreshCommands: async () => {
    const { activeSessionId } = get()
    if (!activeSessionId) return

    try {
      const response = await api.listCommands(activeSessionId)
      const statusItems: Record<string, string> = {}
      for (const item of response.statusItems) {
        statusItems[item.id] = item.text
      }
      set({
        slashCommands: response.commands,
        keybindings: response.keybindings,
        statusItems,
      })
    } catch (err) {
      console.error('Failed to refresh commands:', err)
    }
  },

  submitPrompt: async (text: string) => {
    const { activeSessionId } = get()
    if (!activeSessionId) {
      return false
    }

    const compactCommand = isCompactCommand(text)
    if (compactCommand) {
      set({ compactSubmitting: true, phase: 'compacting' })
    }

    try {
      const response = await api.submitPrompt(activeSessionId, text)
      if (response.kind === 'handled') {
        if (get().activeSessionId !== response.sessionId) {
          return true
        }
        if (response.message === 'compact accepted') {
          await get().refreshSessions()
          await get().switchSession(response.sessionId)
        } else if (response.message === 'queued for next turn') {
          set((current) => ({
            queuedMessages: [...current.queuedMessages, text],
          }))
        } else if (response.message.trim()) {
          set((current) => ({
            blocks: [...current.blocks, commandNoteBlock(response.message)],
          }))
        }
      }
      return true
    } finally {
      if (compactCommand) {
        const current = get()
        set({
          compactSubmitting: false,
          phase: phaseFromControl(current.control),
        })
      }
    }
  },

  abortCurrentTurn: async () => {
    const { activeSessionId } = get()
    if (!activeSessionId) return
    await api.abortSession(activeSessionId)
  },

  applyDelta: (delta: ConversationDelta) => {
    applyDeltaToState(get(), delta, get, set)
  },

  clearTransientHint: () => {
    set({ transientHint: null })
  },
}))

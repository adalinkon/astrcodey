import { getBaseUrl } from './api'
import type {
  ConversationBlock,
  ConversationControlState,
  ConversationCursor,
  ConversationDelta,
  ConversationStreamEnvelope,
  ToolOutputStream,
} from './types'

export type SseEventHandler = (envelope: ConversationStreamEnvelope) => void

type JsonObject = Record<string, unknown>

function isObject(value: unknown): value is JsonObject {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function stringField(
  source: JsonObject,
  camelName: string,
  snakeName?: string
): string | null {
  const value = source[camelName] ?? (snakeName ? source[snakeName] : undefined)
  return typeof value === 'string' ? value : null
}

function normalizeCursor(value: unknown): ConversationCursor | null {
  if (!isObject(value)) return null
  const cursor = stringField(value, 'value')
  return cursor ? { value: cursor } : null
}

function normalizeBlock(value: unknown): ConversationBlock | null {
  if (!isObject(value)) return null
  const kind = stringField(value, 'kind')
  const id = stringField(value, 'id')
  if (!kind || !id) return null

  switch (kind) {
    case 'user': {
      const text = stringField(value, 'text')
      return text != null ? { kind, id, text } : null
    }
    case 'assistant': {
      const text = stringField(value, 'text')
      const status = stringField(value, 'status')
      if (text == null || !isBlockStatus(status)) return null
      return { kind, id, text, status }
    }
    case 'toolCall': {
      const name = stringField(value, 'name')
      const text = stringField(value, 'text')
      const status = stringField(value, 'status')
      if (name == null || text == null || !isBlockStatus(status)) return null
      return { kind, id, name, text, status }
    }
    case 'error': {
      const message = stringField(value, 'message')
      return message != null ? { kind, id, message } : null
    }
    case 'systemNote': {
      const text = stringField(value, 'text')
      return text != null ? { kind, id, text } : null
    }
    default:
      return null
  }
}

function isBlockStatus(
  value: string | null
): value is 'streaming' | 'complete' | 'error' {
  return value === 'streaming' || value === 'complete' || value === 'error'
}

function normalizeToolOutputStream(
  value: string | null
): ToolOutputStream | null {
  if (value === 'stdout' || value === 'stderr') return value
  return null
}

function normalizeDelta(value: unknown): ConversationDelta | null {
  if (!isObject(value)) return null
  const kind = stringField(value, 'kind')

  switch (kind) {
    case 'appendBlock': {
      const block = normalizeBlock(value.block)
      return block ? { kind, block } : null
    }
    case 'patchBlock': {
      const blockId = stringField(value, 'blockId', 'block_id')
      const textDelta = stringField(value, 'textDelta', 'text_delta')
      if (!blockId || textDelta == null) return null
      return { kind, blockId, textDelta }
    }
    case 'finalizeBlock': {
      const block = normalizeBlock(value.block)
      return block ? { kind, block } : null
    }
    case 'completeBlock': {
      const blockId = stringField(value, 'blockId', 'block_id')
      if (!blockId) return null
      const text = stringField(value, 'text')
      return text == null ? { kind, blockId } : { kind, blockId, text }
    }
    case 'updateControlState': {
      const control = value.control
      return isObject(control)
        ? { kind, control: control as unknown as ConversationControlState }
        : null
    }
    case 'rehydrateRequired':
      return { kind }
    case 'sessionContinued': {
      const parentSessionId = stringField(
        value,
        'parentSessionId',
        'parent_session_id'
      )
      const newSessionId = stringField(value, 'newSessionId', 'new_session_id')
      const parentCursor = normalizeCursor(
        value.parentCursor ?? value.parent_cursor
      )
      if (!parentSessionId || !newSessionId || !parentCursor) return null
      return { kind, parentSessionId, newSessionId, parentCursor }
    }
    case 'toolOutput': {
      const callId = stringField(value, 'callId', 'call_id')
      const stream = normalizeToolOutputStream(stringField(value, 'stream'))
      const delta = stringField(value, 'delta')
      if (!callId || !stream || delta == null) return null
      return { kind, callId, stream, delta }
    }
    case 'thinkingDelta': {
      const delta = stringField(value, 'delta')
      return delta == null ? null : { kind, delta }
    }
    default:
      return null
  }
}

function normalizeEnvelope(value: unknown): ConversationStreamEnvelope | null {
  if (!isObject(value)) return null
  const sessionId = stringField(value, 'sessionId', 'session_id')
  const cursor = normalizeCursor(value.cursor)
  const delta = normalizeDelta(value.delta)
  if (!sessionId || !cursor || !delta) return null
  return { sessionId, cursor, delta }
}

export async function consumeSseStream(
  sessionId: string,
  cursor: string | null,
  onEnvelope: SseEventHandler,
  signal: AbortSignal
): Promise<'ended' | 'aborted'> {
  const params = cursor ? `?cursor=${encodeURIComponent(cursor)}` : ''
  const url = `${getBaseUrl()}/api/sessions/${encodeURIComponent(sessionId)}/stream${params}`
  console.debug('[sse] connecting', { url, cursor })

  let response: Response
  try {
    response = await fetch(url, {
      headers: {
        Accept: 'text/event-stream',
        'Cache-Control': 'no-cache',
      },
      signal,
    })
  } catch (err) {
    console.error('[sse] fetch failed', err)
    throw err
  }

  console.debug('[sse] response', { status: response.status, ok: response.ok })

  if (!response.ok) {
    const text = await response.text().catch(() => '')
    console.error('[sse] non-ok response', {
      status: response.status,
      body: text,
    })
    throw new Error(`SSE ${response.status}: ${text}`)
  }

  if (!response.body) {
    throw new Error('SSE response has no body')
  }

  const reader = response.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''
  let dataLines: string[] = []
  let eventType = 'message'

  const flushEvent = () => {
    if (dataLines.length === 0) {
      eventType = 'message'
      return
    }
    const payload = dataLines.join('\n')
    dataLines = []

    if (eventType === 'conversation') {
      try {
        const envelope = normalizeEnvelope(JSON.parse(payload))
        if (!envelope) {
          console.warn('[sse] ignored malformed conversation event', payload)
          return
        }
        console.debug('[sse] event', envelope.delta.kind, envelope.cursor)
        onEnvelope(envelope)
      } catch (err) {
        console.warn('[sse] parse error', err, payload)
      }
    }
    eventType = 'message'
  }

  while (!signal.aborted) {
    const { value, done } = await reader.read()
    if (done) break

    buffer += decoder.decode(value, { stream: true })
    const lines = buffer.split(/\r?\n/)
    buffer = lines.pop() ?? ''

    for (const line of lines) {
      if (line === '') {
        flushEvent()
        continue
      }
      if (line.startsWith(':')) continue
      if (line.startsWith('id:')) {
        continue
      }
      if (line.startsWith('event:')) {
        const nextType = line.slice(6).trimStart()
        eventType = nextType || 'message'
        continue
      }
      if (line.startsWith('data:')) {
        dataLines.push(line.slice(5).trimStart())
      }
    }
  }

  // Flush remaining
  buffer += decoder.decode()
  if (buffer) {
    for (const line of buffer.split(/\r?\n/)) {
      if (line.startsWith('data:')) dataLines.push(line.slice(5).trimStart())
    }
  }
  flushEvent()

  return signal.aborted ? 'aborted' : 'ended'
}

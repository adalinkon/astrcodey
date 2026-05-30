import { useEffect } from 'react'
import { useAppStore } from '../store/conversation'

function normalizeKey(event: KeyboardEvent): string {
  const parts: string[] = []
  if (event.ctrlKey || event.metaKey) parts.push('Ctrl')
  if (event.altKey) parts.push('Alt')
  if (event.shiftKey) parts.push('Shift')
  const key = event.key.length === 1 ? event.key.toUpperCase() : event.key
  if (!['Control', 'Alt', 'Shift', 'Meta'].includes(event.key)) {
    parts.push(key)
  }
  return parts.join('+')
}

function parseBinding(key: string): string {
  return key
    .split('+')
    .map((part) => {
      const trimmed = part.trim()
      if (trimmed.toLowerCase() === 'shift') return 'Shift'
      if (trimmed.toLowerCase() === 'ctrl') return 'Ctrl'
      if (trimmed.toLowerCase() === 'alt') return 'Alt'
      if (trimmed.toLowerCase() === 'meta') return 'Ctrl'
      return trimmed.length === 1 ? trimmed.toUpperCase() : trimmed
    })
    .join('+')
}

export function useKeybindings() {
  const keybindings = useAppStore((s) => s.keybindings)
  const activeSessionId = useAppStore((s) => s.activeSessionId)
  const submitPrompt = useAppStore((s) => s.submitPrompt)

  useEffect(() => {
    if (!activeSessionId || keybindings.length === 0) return

    const handler = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null
      if (
        target &&
        (target.tagName === 'TEXTAREA' ||
          target.tagName === 'INPUT' ||
          target.isContentEditable)
      ) {
        return
      }

      const pressed = normalizeKey(event)
      const binding = keybindings.find(
        (item) => parseBinding(item.key) === pressed
      )
      if (!binding) return

      event.preventDefault()
      const commandText = binding.arguments.trim()
        ? `/${binding.command} ${binding.arguments}`.trim()
        : `/${binding.command}`
      void submitPrompt(commandText)
    }

    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [activeSessionId, keybindings, submitPrompt])
}

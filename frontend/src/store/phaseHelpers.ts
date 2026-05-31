import type { Phase } from '../services/types'

export function isExecutionPhase(phase: Phase, compactSubmitting: boolean): boolean {
  return (
    compactSubmitting ||
    phase === 'thinking' ||
    phase === 'streaming' ||
    phase === 'calling_tool' ||
    phase === 'compacting'
  )
}

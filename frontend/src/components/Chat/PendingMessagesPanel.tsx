import { useMemo, useState } from 'react'
import { useAppStore } from '../../store/conversation'
import type { PendingMessage } from '../../store/types'
import { cn } from '../../lib/utils'
import { chevronIcon, ghostIconButton, pillNeutral } from '../../lib/styles'
import { Icon } from '../ui/Icon'
import { IconButton } from '../ui/IconButton'

interface PendingMessagesPanelProps {
  onEdit: (text: string) => void
  canInject: boolean
}

function pendingSummary(messages: PendingMessage[]): string {
  const queuedCount = messages.filter((item) => item.delivery === 'queued').length
  const injectCount = messages.filter((item) => item.delivery === 'inject').length

  if (injectCount === 0) {
    return `${queuedCount} Queued`
  }
  if (queuedCount === 0) {
    return `${injectCount} Inject`
  }
  return `${messages.length} Pending`
}

function deliveryLabel(delivery: PendingMessage['delivery']): string {
  return delivery === 'queued' ? 'Queued' : 'Inject'
}

export default function PendingMessagesPanel({
  onEdit,
  canInject,
}: PendingMessagesPanelProps) {
  const pendingMessages = useAppStore((s) => s.pendingMessages)
  const togglePendingDelivery = useAppStore((s) => s.togglePendingDelivery)
  const removePendingMessage = useAppStore((s) => s.removePendingMessage)
  const restorePendingMessage = useAppStore((s) => s.restorePendingMessage)
  const [expanded, setExpanded] = useState(true)

  const summary = useMemo(
    () => pendingSummary(pendingMessages),
    [pendingMessages]
  )

  if (pendingMessages.length === 0) {
    return null
  }

  return (
    <div className="mb-2 rounded-2xl border border-border bg-surface-soft/70 px-3 py-2.5 shadow-soft">
      <button
        type="button"
        className="group flex w-full items-center gap-2 text-left text-[12px] font-medium text-text-secondary"
        onClick={() => setExpanded((open) => !open)}
        aria-expanded={expanded}
      >
        <span className={cn(chevronIcon, !expanded && '-rotate-90')}>
          <Icon name="chevron-down" size={14} />
        </span>
        <span>{summary}</span>
      </button>

      {expanded && (
        <ul className="mt-2 space-y-1.5">
          {pendingMessages.map((message) => (
            <li
              key={message.id}
              className="flex items-start gap-2 rounded-xl border border-border/70 bg-white/50 px-2.5 py-2"
            >
              <span
                className="mt-1.5 inline-flex h-2 w-2 shrink-0 rounded-full border border-border-strong/60"
                aria-hidden="true"
              />
              <div className="min-w-0 flex-1">
                <div className="mb-1">
                  <span className={cn(pillNeutral, 'px-2 py-0.5 text-[10px]')}>
                    {deliveryLabel(message.delivery)}
                  </span>
                </div>
                <p className="line-clamp-3 text-[13px] leading-[1.6] text-text-primary">
                  {message.text}
                </p>
              </div>
              <div className="flex shrink-0 items-center gap-0.5">
                <IconButton
                  icon="edit"
                  label="编辑"
                  size={14}
                  onClick={() => {
                    const text = restorePendingMessage(message.id)
                    if (text) onEdit(text)
                  }}
                />
                <button
                  type="button"
                  className={cn(
                    ghostIconButton,
                    'p-1',
                    message.delivery === 'inject' &&
                      'text-accent-strong hover:text-accent-strong'
                  )}
                  aria-label={
                    message.delivery === 'queued'
                      ? '切换为 Inject'
                      : '切换为 Queued'
                  }
                  title={
                    message.delivery === 'queued'
                      ? canInject
                        ? '切换为 Inject'
                        : '当前无活跃 turn，无法 Inject'
                      : '切换为 Queued'
                  }
                  disabled={message.delivery === 'inject' && !canInject}
                  onClick={() => void togglePendingDelivery(message.id)}
                >
                  <Icon name="send" size={14} />
                </button>
                <IconButton
                  icon="trash"
                  label="删除"
                  size={14}
                  onClick={() => removePendingMessage(message.id)}
                />
              </div>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}

import { memo } from 'react'
import type { ConversationBlock } from '../../services/types'
import { cn } from '../../lib/utils'
import { MarkdownContent } from './MarkdownContent'

interface UserMessageProps {
  block: Extract<ConversationBlock, { kind: 'user' }>
}

function UserMessage({ block }: UserMessageProps) {
  return (
    <div className="flex justify-end">
      <div
        className={cn(
          'max-w-[85%] rounded-2xl rounded-br-md border border-user-bubble-border',
          'bg-user-bubble px-4 py-3 text-[15px] leading-[1.65] text-text-primary prose-chat'
        )}
      >
        <MarkdownContent text={block.text} />
      </div>
    </div>
  )
}

export default memo(UserMessage)

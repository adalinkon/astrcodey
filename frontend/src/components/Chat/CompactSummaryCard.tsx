import { memo, useState } from 'react'
import type { ConversationBlock } from '../../services/types'
import { cn } from '../../lib/utils'
import { pillNeutral } from '../../lib/styles'

interface CompactSummaryCardProps {
  block: Extract<ConversationBlock, { kind: 'compactSummary' }>
}

function CompactSummaryCard({ block }: CompactSummaryCardProps) {
  const [expanded, setExpanded] = useState(false)
  const lines = block.summary.split('\n')
  const previewLines = lines.slice(0, 3)
  const hasMore = lines.length > 3
  const ratio =
    block.preTokens > 0
      ? Math.round((block.postTokens / block.preTokens) * 100)
      : 0

  return (
    <div className="rounded-[18px] border border-border bg-surface-soft px-5 py-4 shadow-soft">
      <div className="flex items-center gap-2 text-[13px]">
        <svg
          width="16"
          height="16"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
          className="shrink-0 text-text-muted"
        >
          <polyline points="1 4 1 10 7 10"></polyline>
          <polyline points="23 20 23 14 17 14"></polyline>
          <path d="M20.49 9A9 9 0 0 0 5.64 5.64L1 10m22 4l-4.64 4.36A9 9 0 0 1 3.51 15"></path>
        </svg>
        <span className="font-medium text-text-primary">对话已压缩</span>
        <span className={pillNeutral}>{block.trigger}</span>
        <span className="ml-auto shrink-0 font-mono text-[11px] text-text-muted">
          {block.preTokens.toLocaleString()} &rarr;{' '}
          {block.postTokens.toLocaleString()} tokens{' '}
          <span className="text-text-secondary">({ratio}%)</span>
        </span>
      </div>

      <div
        className={cn(
          'mt-3 cursor-pointer whitespace-pre-wrap text-[13px] leading-relaxed text-text-secondary',
          !expanded && hasMore && 'line-clamp-3'
        )}
        onClick={() => setExpanded(!expanded)}
      >
        {expanded ? block.summary : previewLines.join('\n')}
      </div>

      {hasMore && (
        <button
          className="mt-2 text-[12px] text-text-muted hover:text-text-secondary"
          onClick={() => setExpanded(!expanded)}
        >
          {expanded ? '收起' : '展开全部'}
        </button>
      )}

      {block.transcriptPath && (
        <div className="mt-2 truncate font-mono text-[11px] text-text-muted">
          {block.transcriptPath}
        </div>
      )}
    </div>
  )
}

export default memo(CompactSummaryCard)

import { memo, useState } from 'react'
import type { ConversationBlock } from '../../services/types'
import { cn } from '../../lib/utils'
import {
  extractRenderSpec,
  extractRenderSummary,
} from '../../types/render-spec'
import { chevronIcon, toolPanelPaddingX, toolPanelScrollViewport } from '../../lib/styles'
import { RenderSpecViewer } from './RenderSpecViewer'
import './tools/builtinRenderers'
import {
  getToolRenderer,
  type ToolRenderer,
  type ToolRendererContext,
} from './tools/registry'
import { compactLine, statusLabel, toolArgs, toolMeta } from './tools/helpers'
import {
  buildStreamingAgentSpec,
  DefaultToolDetails,
  StatusIndicatorDot,
} from './tools/shared'
import { Icon } from '../ui/Icon'

interface ToolCallBlockProps {
  block: Extract<ConversationBlock, { kind: 'toolCall' }>
}

function ToolDetails({
  context,
  renderer,
}: {
  context: ToolRendererContext
  renderer?: ToolRenderer
}) {
  if (context.renderSpec) return <RenderSpecViewer spec={context.renderSpec} />
  if (context.agentSpec) return <RenderSpecViewer spec={context.agentSpec} />
  const rendered = renderer?.render?.(context)
  if (rendered != null) return rendered
  return <DefaultToolDetails block={context.block} />
}

function ToolCallBlock({ block }: ToolCallBlockProps) {
  const [isOpen, setIsOpen] = useState(false)
  const args = toolArgs(block)
  const meta = toolMeta(block)

  const renderSpec = extractRenderSpec(block.metadata)
  const agentSpec =
    block.name === 'agent' && block.argumentsJson && !renderSpec
      ? buildStreamingAgentSpec(block.argumentsJson)
      : undefined
  const context: ToolRendererContext = {
    block,
    args,
    meta,
    renderSpec,
    agentSpec,
  }
  const renderer = getToolRenderer(context)

  const summaryLine = compactLine(
    extractRenderSummary(block.metadata) ||
      renderer?.summary?.(context) ||
      block.arguments ||
      block.text ||
      (block.status === 'streaming' ? '等待输出...' : '(无输出)')
  )

  return (
    <details
      className="group mb-1 ml-[var(--layout-assistant-indent)] block min-w-0 max-w-full animate-block-enter motion-reduce:animate-none"
      open={block.status === 'error' || isOpen || !!agentSpec}
      onToggle={(e) => setIsOpen(e.currentTarget.open)}
    >
      <summary className="flex min-w-0 cursor-pointer list-none items-center gap-3 py-2 font-mono text-[13px] leading-relaxed text-text-secondary select-none hover:opacity-90 [&::-webkit-details-marker]:hidden">
        <span className="inline-flex shrink-0 items-center gap-1.5 rounded-md border border-border bg-surface px-2 py-0.5 font-mono text-[11px] font-semibold uppercase tracking-wider text-text-secondary">
          <StatusIndicatorDot status={block.status} />
          {block.name}
        </span>
        <span
          className="block min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[12.5px] text-text-secondary/85 opacity-90"
          title={summaryLine}
        >
          {summaryLine}
        </span>
        <span className="shrink-0 text-[11px] font-semibold uppercase tracking-wider text-text-muted">
          {statusLabel(block.status)}
        </span>
        <span className={chevronIcon}>
          <Icon name="chevron-right" size={14} />
        </span>
      </summary>
      <div className="mt-1.5 min-w-0 overflow-hidden rounded-xl border border-border bg-code-surface shadow-soft">
        <div className={toolPanelScrollViewport}>
          <div className={cn(toolPanelPaddingX, 'py-3')}>
            <ToolDetails context={context} renderer={renderer} />
          </div>
        </div>
      </div>
    </details>
  )
}

export default memo(ToolCallBlock)

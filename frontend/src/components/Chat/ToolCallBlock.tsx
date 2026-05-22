import { memo, useState } from 'react'
import type { ConversationBlock } from '../../services/types'
import { extractRenderSpec } from '../../types/render-spec'
import { chevronIcon } from '../../lib/styles'
import { cn } from '../../lib/utils'
import { RenderSpecViewer } from './RenderSpecViewer'

interface ToolCallBlockProps {
  block: Extract<ConversationBlock, { kind: 'toolCall' }>
}

function statusLabel(status: string): string {
  switch (status) {
    case 'complete':
      return '完成'
    case 'error':
      return '失败'
    case 'backgrounded':
      return '后台运行中'
    default:
      return '运行中'
  }
}

function compactLine(text: string): string {
  return text.replace(/\s+/g, ' ').trim()
}

/**
 * 从 agent 工具的原始 JSON 参数构造 streaming 阶段的 RenderSpec。
 * 直接读结构化字段，不依赖后端格式化字符串。
 */
function stringField(
  obj: Record<string, unknown>,
  camel: string,
  snake?: string
): string {
  const v = obj[camel] ?? (snake ? obj[snake] : undefined)
  return typeof v === 'string' ? v : ''
}

function boolField(
  obj: Record<string, unknown>,
  camel: string
): boolean | undefined {
  const v = obj[camel]
  return typeof v === 'boolean' ? v : undefined
}

function buildStreamingAgentSpec(
  argsJson: Record<string, unknown>
): import('../../types/render-spec').RenderSpec {
  const entries: { key: string; value: string; tone: 'accent' | 'muted' }[] = []

  const description = stringField(argsJson, 'description')
  const agent = stringField(argsJson, 'subagentType', 'subagent_type')
  const model = stringField(argsJson, 'model')
  const rawMode = boolField(argsJson, 'waitForResult')
  const mode = rawMode !== undefined ? (rawMode ? 'sync' : 'async') : ''

  if (description)
    entries.push({ key: 'task', value: description, tone: 'accent' })
  if (agent) entries.push({ key: 'agent', value: agent, tone: 'accent' })
  if (model) entries.push({ key: 'model', value: model, tone: 'muted' })
  if (mode) entries.push({ key: 'mode', value: mode, tone: 'muted' })

  const prompt = stringField(argsJson, 'prompt')

  return {
    type: 'box',
    children: [
      ...(entries.length > 0
        ? [
            {
              type: 'key_value' as const,
              entries,
              tone: undefined as undefined,
            },
          ]
        : []),
      ...(prompt
        ? [
            {
              type: 'text' as const,
              text: `prompt: ${prompt.slice(0, 180)}`,
              tone: 'muted' as const,
            },
          ]
        : []),
    ],
  }
}

function StatusIndicatorDot({ status }: { status: string }) {
  const dotColor =
    status === 'complete'
      ? 'bg-success'
      : status === 'error'
        ? 'bg-danger'
        : 'bg-accent-strong animate-pulse'
  return <span className={cn('h-1.5 w-1.5 rounded-full shrink-0', dotColor)} />
}

function ToolCallBlock({ block }: ToolCallBlockProps) {
  const [isOpen, setIsOpen] = useState(false)

  const renderSpec = extractRenderSpec(
    block.metadata as Record<string, unknown> | undefined
  )

  // 折叠摘要行：显示 LLM 调用的参数（如果有的话），否则回退到结果摘要
  const summaryLine = compactLine(
    block.arguments ||
      block.text ||
      (block.status === 'streaming' ? '等待输出...' : '(无输出)')
  )
  // 展开区域：agent 工具从 JSON 参数构造实时 spec，否则显示结果
  const agentSpec =
    block.name === 'agent' && block.argumentsJson && !renderSpec
      ? buildStreamingAgentSpec(block.argumentsJson)
      : undefined
  const resultText =
    block.text || (block.status === 'streaming' ? '等待输出...' : '')

  return (
    <details
      className="group mb-1 ml-[var(--chat-assistant-content-offset)] block min-w-0 max-w-full animate-block-enter motion-reduce:animate-none"
      open={block.status === 'error' || isOpen || !!agentSpec}
      onToggle={(e) => setIsOpen(e.currentTarget.open)}
    >
      <summary className="flex min-w-0 cursor-pointer items-center gap-3 py-2 font-mono text-[13px] leading-relaxed text-text-secondary list-none [&::-webkit-details-marker]:hidden hover:opacity-90 select-none">
        <span className="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-md bg-surface border border-border font-mono text-[11px] font-semibold text-text-secondary uppercase tracking-wider shrink-0">
          <StatusIndicatorDot status={block.status} />
          {block.name}
        </span>
        <span
          className="block min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-text-secondary/85 text-[12.5px] font-mono opacity-90"
          title={summaryLine}
        >
          {summaryLine}
        </span>
        <span className="shrink-0 text-[11px] font-semibold uppercase tracking-wider text-text-muted">
          {statusLabel(block.status)}
        </span>
        <span className={chevronIcon}>
          <svg
            width="14"
            height="14"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <polyline points="9 18 15 12 9 6"></polyline>
          </svg>
        </span>
      </summary>
      <div className="mt-1.5 flex min-w-0 flex-col rounded-xl border border-border bg-code-surface px-4 py-3 shadow-soft">
        <div className="min-w-0 overflow-y-auto overscroll-contain pr-1 max-h-[min(58vh,560px)]">
          {renderSpec ? (
            <RenderSpecViewer spec={renderSpec} />
          ) : agentSpec ? (
            <RenderSpecViewer spec={agentSpec} />
          ) : (
            <pre className="m-0 overflow-x-auto font-mono text-[13px] leading-relaxed text-code-text">
              <code>{resultText}</code>
            </pre>
          )}
        </div>
      </div>
    </details>
  )
}

export default memo(ToolCallBlock)

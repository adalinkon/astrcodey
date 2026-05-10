import { memo, useMemo, useState } from 'react'
import { useAppStore } from '../../store/conversation'
import { cn } from '../../lib/utils'
import type { ConversationBlock } from '../../services/types'

/** 工具名到人类可读标签的映射 */
const TOOL_LABELS: Record<string, string> = {
  shell: 'Shell',
  read: 'Read',
  write: 'Write',
  edit: 'Edit',
  glob: 'Glob',
  grep: 'Grep',
  task: 'Task',
  agent: 'Agent',
}

function toolLabel(name: string): string {
  return TOOL_LABELS[name] ?? name
}

type ToolCallBlock = Extract<ConversationBlock, { kind: 'toolCall' }>

function hasTaskId(
  block: ConversationBlock
): block is ToolCallBlock & { taskId: string } {
  return block.kind === 'toolCall' && block.taskId !== undefined
}

function BackgroundTaskPanel() {
  const [collapsed, setCollapsed] = useState(true)
  const blocks = useAppStore((s) => s.blocks)

  const bgBlocks = useMemo(() => blocks.filter(hasTaskId), [blocks])

  const running = useMemo(
    () => bgBlocks.filter((b) => b.status === 'backgrounded'),
    [bgBlocks]
  )

  const completed = useMemo(
    () =>
      bgBlocks.filter((b) => b.status === 'complete' || b.status === 'error'),
    [bgBlocks]
  )

  if (bgBlocks.length === 0) return null

  return (
    <div className="shrink-0 border-t border-border bg-surface/80 px-4 py-0 backdrop-blur-[8px]">
      <button
        type="button"
        className="flex w-full items-center gap-2 py-2 text-xs font-medium text-text-secondary hover:text-text-primary transition-colors"
        onClick={() => setCollapsed((v) => !v)}
        aria-expanded={!collapsed}
      >
        <svg
          className={cn(
            'h-3 w-3 shrink-0 transition-transform duration-150',
            collapsed ? '' : 'rotate-90'
          )}
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <polyline points="9 18 15 12 9 6" />
        </svg>
        <span className="flex items-center gap-1.5">
          {running.length > 0 && (
            <>
              <span className="inline-block h-1.5 w-1.5 rounded-full bg-accent animate-pulse" />
              <span>{running.length} 个后台任务运行中</span>
            </>
          )}
          {running.length === 0 && completed.length > 0 && (
            <span>{completed.length} 个后台任务已完成</span>
          )}
        </span>
      </button>

      {!collapsed && (
        <div className="flex flex-col gap-1 pb-2">
          {bgBlocks.map((block) => (
            <div
              key={block.id}
              className="flex items-center gap-2 rounded-lg px-2 py-1.5 text-xs hover:bg-black/[0.02]"
            >
              <span
                className={cn(
                  'inline-block h-1.5 w-1.5 shrink-0 rounded-full',
                  block.status === 'backgrounded' && 'bg-accent animate-pulse',
                  block.status === 'complete' && 'bg-green-500',
                  block.status === 'error' && 'bg-red-500'
                )}
              />
              <span className="font-medium text-text-primary">
                {toolLabel(block.name)}
              </span>
              <span className="min-w-0 flex-1 truncate text-text-secondary">
                {block.status === 'backgrounded'
                  ? '运行中...'
                  : block.status === 'error'
                    ? '失败'
                    : '完成'}
              </span>
              <span className="shrink-0 font-mono text-[10px] text-text-muted">
                {block.taskId?.slice(0, 8) ?? '—'}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

export default memo(BackgroundTaskPanel)

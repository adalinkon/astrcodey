import { cn } from '../../lib/utils'

interface DiffCodeLinesProps {
  text: string
  className?: string
  lineClassName?: string
}

export function DiffCodeLines({
  text,
  className,
  lineClassName,
}: DiffCodeLinesProps) {
  return (
    <code className={cn('block min-w-0', className)}>
      {text.split('\n').map((line, index) => {
        const isFileHeader = line.startsWith('+++') || line.startsWith('---')
        const isAddition = line.startsWith('+') && !isFileHeader
        const isDeletion = line.startsWith('-') && !isFileHeader
        const isHunk = line.startsWith('@@')

        return (
          <span
            key={index}
            className={cn(
              'block min-w-fit',
              lineClassName,
              isAddition && 'bg-success-soft/70 text-success',
              isDeletion && 'bg-danger-soft/70 text-danger',
              isFileHeader && 'text-text-muted',
              isHunk && 'bg-surface text-text-secondary'
            )}
          >
            {line || ' '}
          </span>
        )
      })}
    </code>
  )
}

import type { ReactNode } from 'react'
import { cn } from '../../lib/utils'

interface ContentColumnProps {
  children: ReactNode
  className?: string
  /** When true, content is constrained to max-width and centered */
  constrained?: boolean
}

export function ContentColumn({
  children,
  className,
  constrained = true,
}: ContentColumnProps) {
  return (
    <div
      className={cn(
        'w-full min-w-0',
        constrained &&
          'mx-auto max-w-[var(--layout-content-max-width)] px-[var(--layout-page-padding-x)]',
        className
      )}
    >
      {children}
    </div>
  )
}

import type { ReactNode } from 'react'
import { cn } from '../../lib/utils'

interface PageHeaderProps {
  children: ReactNode
  className?: string
}

export function PageHeader({ children, className }: PageHeaderProps) {
  return (
    <header
      className={cn(
        'relative z-30 shrink-0 border-b border-border bg-surface/92 backdrop-blur-[12px]',
        className
      )}
    >
      <div className="flex items-center gap-4 px-[var(--layout-page-padding-x)] py-3.5">
        {children}
      </div>
    </header>
  )
}

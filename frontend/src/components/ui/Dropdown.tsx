import { useEffect, useRef, type ReactNode } from 'react'
import { cn } from '../../lib/utils'

interface DropdownProps {
  open: boolean
  onClose: () => void
  trigger: ReactNode
  children: ReactNode
  className?: string
  align?: 'left' | 'right'
  label?: string
}

export function Dropdown({
  open,
  onClose,
  trigger,
  children,
  className,
  align = 'right',
  label,
}: DropdownProps) {
  const menuRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const handler = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        onClose()
      }
    }
    document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [open, onClose])

  return (
    <div ref={menuRef} className="relative shrink-0">
      {trigger}
      {open && (
        <div
          role="menu"
          aria-label={label}
          className={cn(
            'absolute top-full z-50 mt-1 min-w-[220px] max-w-[360px] rounded-lg border border-border bg-surface p-2 shadow-surface-lg',
            align === 'right' ? 'right-0' : 'left-0',
            className
          )}
        >
          {children}
        </div>
      )}
    </div>
  )
}

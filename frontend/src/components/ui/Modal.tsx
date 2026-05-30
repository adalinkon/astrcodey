import { useEffect, useRef, type ReactNode } from 'react'
import { cn } from '../../lib/utils'
import { dialogSurface, overlay } from '../../lib/styles'
import { IconButton } from './IconButton'

interface ModalProps {
  title: string
  children: ReactNode
  onClose: () => void
  className?: string
  closeOnOverlay?: boolean
}

export function Modal({
  title,
  children,
  onClose,
  className,
  closeOnOverlay = true,
}: ModalProps) {
  const dialogRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [onClose])

  useEffect(() => {
    dialogRef.current?.focus()
  }, [])

  return (
    <div
      className={overlay}
      role="presentation"
      onClick={(e) => {
        if (closeOnOverlay && e.target === e.currentTarget) onClose()
      }}
    >
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby="modal-title"
        tabIndex={-1}
        className={cn(dialogSurface, className)}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="mb-[18px] flex items-center justify-between gap-3">
          <h2 id="modal-title" className="text-xl font-bold text-text-primary">
            {title}
          </h2>
          <IconButton icon="close" label="关闭" onClick={onClose} />
        </div>
        {children}
      </div>
    </div>
  )
}

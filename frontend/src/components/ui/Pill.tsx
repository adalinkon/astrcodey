import type { ReactNode } from 'react'
import { cn } from '../../lib/utils'
import {
  pillBase,
  pillDanger,
  pillNeutral,
  pillSuccess,
} from '../../lib/styles'

type PillTone = 'neutral' | 'success' | 'danger'

interface PillProps {
  tone?: PillTone
  children: ReactNode
  className?: string
}

const toneClass: Record<PillTone, string> = {
  neutral: pillNeutral,
  success: pillSuccess,
  danger: pillDanger,
}

export function Pill({ tone = 'neutral', children, className }: PillProps) {
  return (
    <span className={cn(pillBase, toneClass[tone], className)}>{children}</span>
  )
}

import type { ButtonHTMLAttributes } from 'react'
import { cn } from '../../lib/utils'
import { ghostIconButton } from '../../lib/styles'
import { Icon, type IconName } from './Icon'

interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  icon: IconName
  label: string
  size?: number
}

export function IconButton({
  icon,
  label,
  size = 16,
  className,
  ...props
}: IconButtonProps) {
  return (
    <button
      type="button"
      className={cn(ghostIconButton, 'p-1', className)}
      aria-label={label}
      title={label}
      {...props}
    >
      <Icon name={icon} size={size} />
    </button>
  )
}

import type { ButtonHTMLAttributes, ReactNode } from 'react'
import { cn } from '../../lib/utils'
import { btnPrimary, btnSecondary, ghostIconButton } from '../../lib/styles'

type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger'

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant
  children: ReactNode
}

const variantClass: Record<ButtonVariant, string> = {
  primary: btnPrimary,
  secondary: btnSecondary,
  ghost: ghostIconButton,
  danger:
    'rounded-xl border border-danger bg-danger-soft px-4 py-2.5 text-[13px] font-semibold text-danger transition-[filter,opacity,transform] duration-150 ease-out hover:brightness-98 active:scale-[0.98]',
}

export function Button({
  variant = 'secondary',
  className,
  children,
  type = 'button',
  ...props
}: ButtonProps) {
  return (
    <button
      type={type}
      className={cn(variantClass[variant], className)}
      {...props}
    >
      {children}
    </button>
  )
}

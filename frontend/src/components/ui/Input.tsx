import type { InputHTMLAttributes } from 'react'
import { cn } from '../../lib/utils'
import { fieldInput } from '../../lib/styles'

type InputProps = InputHTMLAttributes<HTMLInputElement>

export function Input({ className, ...props }: InputProps) {
  return <input className={cn(fieldInput, className)} {...props} />
}

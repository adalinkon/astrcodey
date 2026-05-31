import { useCallback } from 'react'
import { useAppStore } from '../store/conversation'
import { btnPrimary } from '../lib/styles'
import { Modal } from './ui'

function hintTitle(message: string): string {
  if (message.includes('inject') || message.includes('Inject')) {
    return '无法 Inject'
  }
  if (message.includes('发送失败') || message.includes('失败')) {
    return '操作失败'
  }
  if (message.includes('接口不存在')) {
    return '服务版本过旧'
  }
  return '提示'
}

export default function TransientHintDialog() {
  const hint = useAppStore((s) => s.transientHint)
  const clearTransientHint = useAppStore((s) => s.clearTransientHint)

  const close = useCallback(() => {
    clearTransientHint()
  }, [clearTransientHint])

  if (!hint) {
    return null
  }

  return (
    <Modal title={hintTitle(hint)} onClose={close} className="w-[420px]">
      <p className="mb-6 text-[14px] leading-relaxed text-text-secondary">
        {hint}
      </p>
      <div className="flex justify-end">
        <button type="button" className={btnPrimary} onClick={close}>
          知道了
        </button>
      </div>
    </Modal>
  )
}

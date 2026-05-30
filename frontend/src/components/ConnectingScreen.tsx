import { useAppStore } from '../store/conversation'
import { errorSurface } from '../lib/styles'
import { Button } from '../components/ui'

export default function ConnectingScreen() {
  const status = useAppStore((s) => s.connectionStatus)
  const error = useAppStore((s) => s.connectionError)
  const initServer = useAppStore((s) => s.initServer)

  if (status === 'connected') return null

  return (
    <div className="flex h-full w-full items-center justify-center bg-panel-bg">
      <div className="max-w-md px-6 text-center">
        {status === 'connecting' && (
          <>
            <div className="mb-4">
              <div
                className="inline-flex h-10 w-10 animate-spin rounded-full border-4 border-border border-t-accent-strong"
                role="status"
                aria-label="正在连接"
              />
            </div>
            <div className="text-[15px] font-medium text-text-primary">
              正在启动 AstrCode 服务...
            </div>
            <div className="mt-2 text-[13px] text-text-secondary">
              首次启动可能需要几秒钟
            </div>
          </>
        )}
        {status === 'error' && (
          <>
            <div className={errorSurface}>
              <div className="mb-1.5 text-[13px] font-semibold">连接失败</div>
              <div className="break-words text-xs">{error ?? '未知错误'}</div>
            </div>
            <Button
              variant="secondary"
              className="mt-4"
              onClick={() => void initServer()}
            >
              重试
            </Button>
          </>
        )}
        {status === 'disconnected' && (
          <>
            <div className="text-[15px] font-medium text-text-primary">
              准备就绪
            </div>
            <Button
              variant="secondary"
              className="mt-4"
              onClick={() => void initServer()}
            >
              连接服务
            </Button>
          </>
        )}
      </div>
    </div>
  )
}

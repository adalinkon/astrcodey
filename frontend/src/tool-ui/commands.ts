/**
 * Tool UI → Host command 边界。
 * 插件/内置 UI 只调这里，不直接 fetch 执行工具。
 * 见 docs/tool-ui-architecture.md
 */

export type ToolApprovalRespondPayload = {
  sessionId: string
  toolCallId: string
  /** 工具名，供宿主路由（如 askUser）。 */
  toolName: string
  answers: Record<string, string>
}

/**
 * 用户完成 Approval UI（如 askUser 问卷）后提交。
 * 后端待实现：POST …/tool-ui/respond → tool.approval.respond → ToolCallCompleted。
 */
export async function submitToolApprovalRespond(
  payload: ToolApprovalRespondPayload
): Promise<{ accepted: boolean }> {
  const { submitToolUiRespond } = await import('../services/api')
  return submitToolUiRespond(
    payload.sessionId,
    payload.toolCallId,
    payload.toolName,
    payload.answers
  )
}

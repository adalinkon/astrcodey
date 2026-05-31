import type { SessionListItem } from '../../services/types'

function earliestCreatedAt(sessions: SessionListItem[]): string {
  return sessions.reduce(
    (min, session) => (session.createdAt < min ? session.createdAt : min),
    sessions[0].createdAt
  )
}

/** 按各文件夹内最早会话的 createdAt 升序排列（先创建的在前）。 */
export function computeInitialProjectFolderOrder(
  sessions: SessionListItem[]
): string[] {
  const byWorkingDir = new Map<string, SessionListItem[]>()
  for (const session of sessions) {
    const group = byWorkingDir.get(session.workingDir)
    if (group) {
      group.push(session)
    } else {
      byWorkingDir.set(session.workingDir, [session])
    }
  }

  return [...byWorkingDir.entries()]
    .sort(([, aSessions], [, bSessions]) =>
      earliestCreatedAt(aSessions).localeCompare(earliestCreatedAt(bSessions))
    )
    .map(([workingDir]) => workingDir)
}

/**
 * 保持已有顺序；移除已删项目；新文件夹追加到末尾（不重新排序）。
 */
export function syncProjectFolderOrder(
  currentOrder: string[],
  sessions: SessionListItem[]
): string[] {
  const activeDirs = new Set(sessions.map((session) => session.workingDir))
  const next = currentOrder.filter((workingDir) => activeDirs.has(workingDir))

  for (const session of sessions) {
    if (!next.includes(session.workingDir)) {
      next.push(session.workingDir)
    }
  }

  return next
}

export function groupSessionsByWorkingDir(
  sessions: SessionListItem[]
): Map<string, SessionListItem[]> {
  const groups = new Map<string, SessionListItem[]>()
  for (const session of sessions) {
    const existing = groups.get(session.workingDir)
    if (existing) {
      existing.push(session)
    } else {
      groups.set(session.workingDir, [session])
    }
  }
  return groups
}

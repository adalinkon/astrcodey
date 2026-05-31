/** 统计 text 中 ``` 出现次数（奇数表示仍在代码块内）。 */
export function fenceCount(text: string): number {
  const matches = text.match(/```/g)
  return matches ? matches.length : 0
}

/**
 * Streaming 时在安全边界切分：优先段落（双换行），若在未闭合 fence 内则整段保持纯文本。
 */
export function findStreamingCommitIndex(text: string): number {
  if (!text.includes('\n')) return -1

  if (fenceCount(text) % 2 === 1) {
    const fenceStart = text.lastIndexOf('```')
    if (fenceStart <= 0) return -1
    const beforeFence = text.slice(0, fenceStart)
    const paragraphBreak = beforeFence.lastIndexOf('\n\n')
    if (paragraphBreak !== -1) return paragraphBreak + 1
    return beforeFence.lastIndexOf('\n')
  }

  const paragraphBreak = text.lastIndexOf('\n\n')
  if (paragraphBreak !== -1) return paragraphBreak + 1

  return text.lastIndexOf('\n')
}

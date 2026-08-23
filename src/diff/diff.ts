/** Tiny word-level LCS diff for inline rename display. */

export type DiffPart = {
  type: 'equal' | 'del' | 'ins'
  text: string
}

function tokenize(s: string): string[] {
  return s.split(/(\s+|\/)/).filter((t) => t.length > 0)
}

function lcsMatrix(a: string[], b: string[]): number[][] {
  const m = a.length
  const n = b.length
  const dp: number[][] = Array.from({ length: m + 1 }, () => Array(n + 1).fill(0))
  for (let i = m - 1; i >= 0; i--) {
    for (let j = n - 1; j >= 0; j--) {
      dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1])
    }
  }
  return dp
}

export function diffWords(oldStr: string, newStr: string): DiffPart[] {
  if (oldStr === newStr) return [{ type: 'equal', text: oldStr }]
  const a = tokenize(oldStr)
  const b = tokenize(newStr)
  const dp = lcsMatrix(a, b)
  const parts: DiffPart[] = []
  let i = 0
  let j = 0
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) {
      parts.push({ type: 'equal', text: a[i] })
      i++
      j++
    } else if (dp[i + 1][j] >= dp[i][j + 1]) {
      parts.push({ type: 'del', text: a[i] })
      i++
    } else {
      parts.push({ type: 'ins', text: b[j] })
      j++
    }
  }
  while (i < a.length) parts.push({ type: 'del', text: a[i++] })
  while (j < b.length) parts.push({ type: 'ins', text: b[j++] })
  const merged: DiffPart[] = []
  for (const p of parts) {
    const last = merged[merged.length - 1]
    if (last && last.type === p.type) last.text += p.text
    else merged.push({ ...p })
  }
  return merged
}

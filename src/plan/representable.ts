/** Paths that cannot live in a line-oriented plan.md (no encoding). */
export function isLineSafePath(p: string): boolean {
  return !p.includes('\0') && !p.includes('\r') && !p.includes('\n')
}

export function unrepresentableReason(p: string): string | null {
  if (p.includes('\0')) return 'NUL byte'
  if (p.includes('\r')) return 'CR'
  if (p.includes('\n')) return 'LF'
  return null
}

export function isAbsolutePosix(p: string): boolean {
  if (!p.startsWith('/')) return false
  const parts = p.split('/')
  for (let i = 1; i < parts.length; i++) {
    const seg = parts[i]
    if (seg === '' || seg === '.' || seg === '..') return false
  }
  return true
}

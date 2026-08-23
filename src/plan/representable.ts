const COMMENT_OPEN = '<!--'
const COMMENT_CLOSE = '-->'

export function isRepresentablePath(p: string): boolean {
  if (p.includes('\0') || p.includes('\r') || p.includes('\n') || p.includes('`')) return false
  if (p.includes(COMMENT_OPEN) || p.includes(COMMENT_CLOSE)) return false
  if (p.startsWith(' ') || p.startsWith('\t') || p.endsWith(' ') || p.endsWith('\t')) return false
  const parts = p.split('/')
  for (const part of parts) {
    if (part === '') continue
    if (part.startsWith(' ') || part.startsWith('\t') || part.endsWith(' ') || part.endsWith('\t')) {
      return false
    }
  }
  return true
}

export function isAbsolutePosix(p: string): boolean {
  if (!p.startsWith('/')) return false
  const parts = p.split('/')
  // first part empty (leading /)
  for (let i = 1; i < parts.length; i++) {
    const seg = parts[i]
    if (seg === '' || seg === '.' || seg === '..') return false
  }
  return true
}

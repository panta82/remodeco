import { ParseError, type ParsedBullet, type ParsedPlan, type PlanHeader } from './types.ts'
export { ParseError } from './types.ts'
import { isAbsolutePosix, isLineSafePath } from './representable.ts'
import { parseYamlSingleQuoted } from './frontMatter.ts'

export const MAX_FILE_BYTES = 32 * 1024 * 1024
export const MAX_LINE_BYTES = 64 * 1024
export const MAX_LINES = 200_000
export const MAX_DEST_BYTES = 4096

export const BULLET_RE = /^-\s+`\{(\d+)\}`\t(.*)$/

/** Line-oriented comments so dests may contain `<!--` / backticks. */
export function dropCommentLines(lines: string[]): string[] {
  const out: string[] = []
  let inBlock = false
  for (const line of lines) {
    if (inBlock) {
      if (line.includes('-->')) inBlock = false
      continue
    }
    const t = line.trimStart()
    if (t.startsWith('<!--')) {
      if (!t.includes('-->')) inBlock = true
      continue
    }
    out.push(line)
  }
  if (inBlock) throw new ParseError('unterminated-html-comment')
  return out
}

function splitRawLines(raw: string): string[] {
  if (raw.includes('\r')) throw new ParseError('cr', 'plan.md contains CR; save as LF')
  return raw.split('\n')
}

export function parsePlan(raw: string, rawByteLength?: number): ParsedPlan {
  const size = rawByteLength ?? Buffer.byteLength(raw, 'utf8')
  if (size > MAX_FILE_BYTES) throw new ParseError('plan-too-large', 'plan too large')

  const rawLines = splitRawLines(raw)
  if (rawLines.length > MAX_LINES) throw new ParseError('too-many-lines', 'plan too large')
  for (const line of rawLines) {
    if (Buffer.byteLength(line, 'utf8') > MAX_LINE_BYTES) {
      throw new ParseError('line-too-long', 'line too long')
    }
  }

  const lines = dropCommentLines(rawLines)

  let header: PlanHeader | null = null
  let i = 0
  if (lines[0] === '---') {
    i = 1
    const fm: Record<string, string> = {}
    while (i < lines.length && lines[i] !== '---') {
      const m = /^([A-Za-z0-9_-]+):\s*(.*)$/.exec(lines[i] ?? '')
      if (m) fm[m[1]] = m[2]
      i++
    }
    if (lines[i] === '---') i++
    const remodeco = Number(fm.remodeco ?? '0')
    header = {
      remodeco,
      id: fm.id ?? '',
      root: parseYamlSingleQuoted(fm.root ?? ''),
    }
  }

  const bullets: ParsedBullet[] = []
  const unknownLines: { line: number; text: string }[] = []
  const seen = new Set<number>()

  for (let lineNo = 1; lineNo <= lines.length; lineNo++) {
    const text = lines[lineNo - 1] ?? ''
    if (lineNo - 1 < i) continue
    if (text.trim() === '') continue
    if (/^#{1,6}\s/.test(text)) continue
    const m = BULLET_RE.exec(text)
    if (!m) {
      unknownLines.push({ line: lineNo, text })
      continue
    }
    const id = Number(m[1])
    const destRaw = m[2]
    if (seen.has(id)) throw new ParseError('duplicate-id', `duplicate id ${id}`)
    seen.add(id)
    if (Buffer.byteLength(destRaw, 'utf8') > MAX_DEST_BYTES) {
      throw new ParseError('dest-too-long', 'dest too long')
    }
    if (destRaw === '') {
      bullets.push({ id, destRaw, kind: 'trash', line: lineNo })
      continue
    }
    if (!isLineSafePath(destRaw) || !isAbsolutePosix(destRaw)) {
      throw new ParseError('bad-dest', `unrepresentable or non-absolute dest for {${id}}`)
    }
    bullets.push({ id, destRaw, kind: 'dest', line: lineNo })
  }

  return { header, bullets, skippedIds: [], unknownLines }
}

export function classifyOps(
  plan: ParsedPlan,
  manifest: Map<number, string>,
  mode: 'move' | 'copy',
): {
  ops: { id: number; from: string; to: string; kind: 'skip' | 'noop' | 'trash' | 'move' | 'copy' }[]
  skipped: number[]
} {
  const active = new Set(plan.bullets.map((b) => b.id))
  const skipped: number[] = []
  for (const id of manifest.keys()) {
    if (!active.has(id)) skipped.push(id)
  }
  const ops = plan.bullets.map((b) => {
    const from = manifest.get(b.id)
    if (from === undefined) throw new ParseError('unknown-id', `id {${b.id}} not in manifest`)
    if (b.kind === 'trash') return { id: b.id, from, to: '', kind: 'trash' as const }
    if (b.destRaw === from) return { id: b.id, from, to: b.destRaw, kind: 'noop' as const }
    return { id: b.id, from, to: b.destRaw, kind: mode }
  })
  return { ops, skipped }
}

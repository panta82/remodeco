import path from 'node:path'
import { emitFrontMatter } from './frontMatter.ts'

export type GenerateRow = {
  id: number
  from: string
}

export function padId(id: number, width: number): string {
  return String(id).padStart(width, '0')
}

export function idWidthForCount(n: number): number {
  return Math.max(1, String(Math.max(n, 1)).length)
}

export function generatePlanMd(opts: {
  sessionId: string
  root: string
  rows: GenerateRow[]
  idWidth: number
}): string {
  const { sessionId, root, rows, idWidth } = opts
  const chunks: string[] = [emitFrontMatter(sessionId, root)]
  chunks.push(`# ${root}\n`)

  let curParent: string | null = null
  for (const row of rows) {
    const parent = path.posix.dirname(row.from)
    const rel =
      parent === root ? '.' : parent.startsWith(root + '/') ? parent.slice(root.length + 1) : parent
    if (rel !== curParent) {
      curParent = rel
      chunks.push(`\n## ${rel}\n`)
    }
    chunks.push(`- \`{${padId(row.id, idWidth)}}\`\t${row.from}\n`)
  }
  return chunks.join('')
}

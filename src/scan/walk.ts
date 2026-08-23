import fs from 'node:fs'
import path from 'node:path'
import picomatch from 'picomatch'
import { unrepresentableReason } from '../plan/representable.ts'
import { fingerprintFromStats, type SourceFingerprint } from './fingerprint.ts'

export const DEFAULT_EXCLUDES = ['.git', 'node_modules', '.hg', '.svn']

export type ScanRow = SourceFingerprint & {
  from: string
}

export type ScanResult = {
  root: string
  rows: ScanRow[]
  skippedSpecial: number
}

export class ScanError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message)
    this.name = 'ScanError'
  }
}

const decoder = new TextDecoder('utf-8', { fatal: true })

function decodeName(buf: Buffer): string {
  try {
    return decoder.decode(buf)
  } catch {
    throw new ScanError('invalid-utf8', 'unrepresentable filename (invalid UTF-8)')
  }
}

function excluded(
  basename: string,
  relPosix: string,
  patterns: string[],
): boolean {
  for (const p of patterns) {
    if (basename === p) return true
    if (picomatch.isMatch(basename, p) || picomatch.isMatch(relPosix, p)) return true
  }
  return false
}

export function scanTree(opts: {
  root: string
  recursive: boolean
  hidden: boolean
  exclude: string[]
}): ScanResult {
  const root = fs.realpathSync(opts.root)
  const rootWhy = unrepresentableReason(root)
  if (rootWhy) {
    throw new ScanError('unrepresentable-root', `unrepresentable root path (${rootWhy}): ${root}`)
  }
  const rows: ScanRow[] = []
  let skippedSpecial = 0

  function walk(dir: string): void {
    let entries: fs.Dirent[]
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true, encoding: 'buffer' }) as unknown as fs.Dirent[]
    } catch (e) {
      throw e
    }
    for (const ent of entries) {
      const rawName = (ent as unknown as { name: Buffer }).name
      const name = typeof rawName === 'string' ? rawName : decodeName(Buffer.from(rawName))
      const abs = path.join(dir, name)
      const why = unrepresentableReason(abs) ?? unrepresentableReason(name)
      if (why) {
        throw new ScanError('unrepresentable', `unrepresentable path (${why}): ${abs}`)
      }
      const rel = abs.startsWith(root + path.sep) ? abs.slice(root.length + 1).split(path.sep).join('/') : name
      if (!opts.hidden && name.startsWith('.')) {
        if (ent.isDirectory() && !ent.isSymbolicLink()) continue
        if (!ent.isDirectory()) continue
      }
      if (excluded(name, rel, opts.exclude)) {
        continue
      }
      const st = fs.lstatSync(abs, { bigint: true })
      if (st.isSymbolicLink()) {
        rows.push({ from: abs, ...fingerprintFromStats(st, 'symlink') })
        continue
      }
      if (st.isDirectory()) {
        if (opts.recursive) walk(abs)
        continue
      }
      if (st.isFile()) {
        rows.push({ from: abs, ...fingerprintFromStats(st, 'file') })
        continue
      }
      skippedSpecial++
      console.error(`skipping special file: ${abs}`)
    }
  }

  walk(root)
  rows.sort((a, b) => a.from.localeCompare(b.from, undefined, { numeric: true, sensitivity: 'accent' }))
  return { root, rows, skippedSpecial }
}

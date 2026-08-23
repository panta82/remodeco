import fs from 'node:fs'
import path from 'node:path'
import { dirIdentityFromStats, type DirectoryIdentity } from '../scan/fingerprint.ts'

export type DestParentRef =
  | { kind: 'existing'; identity: DirectoryIdentity; parentPath: string; suffix: string[] }
  | { kind: 'mkdirStep'; stepId: string; parentPath: string; suffix: string[] }

export function splitAbs(p: string): string[] {
  return p.split('/').filter((s) => s.length > 0)
}

/** Deepest existing ancestor of dirname(to), never realpath the dest leaf. */
export function resolveDestParent(to: string): { parentPath: string; suffix: string[]; identity: DirectoryIdentity } {
  const leaf = path.posix.basename(to)
  let dir = path.posix.dirname(to)
  const missing: string[] = [leaf]
  while (dir !== '/' && dir !== '.') {
    try {
      const st = fs.lstatSync(dir, { bigint: true })
      if (st.isSymbolicLink() || !st.isDirectory()) {
        missing.unshift(path.posix.basename(dir))
        dir = path.posix.dirname(dir)
        continue
      }
      return {
        parentPath: dir,
        suffix: missing,
        identity: dirIdentityFromStats(st),
      }
    } catch {
      missing.unshift(path.posix.basename(dir))
      dir = path.posix.dirname(dir)
    }
  }
  const st = fs.lstatSync('/', { bigint: true })
  return { parentPath: '/', suffix: missing, identity: dirIdentityFromStats(st) }
}

export function destOutsideRoot(root: string, to: string): boolean {
  const rootSlash = root.endsWith('/') ? root : root + '/'
  return to !== root && !to.startsWith(rootSlash)
}

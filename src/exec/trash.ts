import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { renameNoReplace } from '../native/koffi.ts'

export type TrashKey = { filesPath: string; infoPath: string | null; originalPath: string }

function mode0777(st: fs.BigIntStats): number {
  return Number(st.mode) & 0o777
}

function isSafeUserTrashDir(p: string, uid: number): boolean {
  const st = fs.lstatSync(p, { bigint: true })
  if (st.isSymbolicLink() || !st.isDirectory()) return false
  if (Number(st.uid) !== uid) return false
  return mode0777(st) === 0o700
}

function isValidSharedDotTrash(p: string): boolean {
  try {
    const st = fs.lstatSync(p, { bigint: true })
    if (st.isSymbolicLink() || !st.isDirectory()) return false
    return (Number(st.mode) & 0o1000) !== 0
  } catch {
    return false
  }
}

function deepestExisting(p: string): string {
  let cur = p
  while (cur !== '/' && !fs.existsSync(cur)) cur = path.dirname(cur)
  return cur
}

function mkdir0700(p: string): void {
  fs.mkdirSync(p, { mode: 0o700 })
  if (!isSafeUserTrashDir(p, process.getuid?.() ?? 0)) {
    throw new Error(`unsafe trash dir ${p}`)
  }
}

function ensureUserTrashTree(root: string, uid: number): void {
  if (!fs.existsSync(root)) mkdir0700(root)
  else if (!isSafeUserTrashDir(root, uid)) throw new Error(`unsafe trash root ${root}`)
  for (const sub of ['files', 'info']) {
    const d = path.join(root, sub)
    if (!fs.existsSync(d)) mkdir0700(d)
    else if (!isSafeUserTrashDir(d, uid)) throw new Error(`unsafe trash ${d}`)
  }
}

function mountPoint(abs: string): string {
  const srcDev = fs.lstatSync(abs, { bigint: true }).dev
  let cur = path.dirname(abs)
  let last = abs
  while (cur !== last) {
    const st = fs.lstatSync(cur, { bigint: true })
    if (st.dev !== srcDev) return last
    last = cur
    cur = path.dirname(cur)
  }
  return last
}

function percentEncode(s: string): string {
  return encodeURIComponent(s).replace(/'/g, '%27')
}

export function locateTrash(source: string): { root: string; kind: 'home' | 'top' | 'macos' } {
  const uid = process.getuid?.() ?? 0
  if (process.platform === 'darwin') {
    const t = path.join(os.homedir(), '.Trash')
    if (!fs.existsSync(t)) fs.mkdirSync(t, { recursive: true })
    return { root: t, kind: 'macos' }
  }
  const xdg =
    process.env.XDG_DATA_HOME && process.env.XDG_DATA_HOME.length
      ? process.env.XDG_DATA_HOME
      : path.join(os.homedir(), '.local', 'share')
  const ancestor = deepestExisting(xdg)
  const srcDev = fs.lstatSync(source, { bigint: true }).dev
  if (!fs.existsSync(xdg)) {
    const parts = path.relative(ancestor, xdg).split(path.sep).filter(Boolean)
    let cur = ancestor
    for (const part of parts) {
      cur = path.join(cur, part)
      if (!fs.existsSync(cur)) mkdir0700(cur)
    }
  }
  const homeTrashDev = fs.lstatSync(xdg, { bigint: true }).dev
  if (srcDev === homeTrashDev) {
    const root = path.join(xdg, 'Trash')
    ensureUserTrashTree(root, uid)
    return { root, kind: 'home' }
  }
  const mount = mountPoint(source)
  const shared = path.join(mount, '.Trash')
  if (fs.existsSync(shared) && isValidSharedDotTrash(shared)) {
    const root = path.join(shared, String(uid))
    ensureUserTrashTree(root, uid)
    return { root, kind: 'top' }
  }
  const root = path.join(mount, `.Trash-${uid}`)
  ensureUserTrashTree(root, uid)
  return { root, kind: 'top' }
}

export function uniqueTrashName(root: string, base: string, macos: boolean): { filesPath: string; infoPath: string | null } {
  const filesDir = macos ? root : path.join(root, 'files')
  const infoDir = macos ? null : path.join(root, 'info')
  let name = base
  let n = 0
  for (;;) {
    const filesPath = path.join(filesDir, name)
    const infoPath = infoDir ? path.join(infoDir, `${name}.trashinfo`) : null
    const filesFree = !exists(filesPath)
    const infoFree = !infoPath || !exists(infoPath)
    if (filesFree && infoFree) return { filesPath, infoPath }
    n++
    name = `${base}.${n}`
  }
}

function exists(p: string): boolean {
  try {
    fs.lstatSync(p)
    return true
  } catch {
    return false
  }
}

export function writeTrashInfo(opts: {
  infoPath: string
  originalPath: string
  mount: string | null
  kind: 'home' | 'top'
  journalId: string
}): void {
  let pathField: string
  if (opts.kind === 'top' && opts.mount) {
    const rel = opts.originalPath.startsWith(opts.mount + '/')
      ? opts.originalPath.slice(opts.mount.length + 1)
      : opts.originalPath.replace(/^\//, '')
    pathField = percentEncode(rel).replace(/%2F/gi, '/')
  } else {
    pathField = percentEncode(opts.originalPath).replace(/%2F/gi, '/')
  }
  const now = new Date()
  const pad = (n: number) => String(n).padStart(2, '0')
  const deletion = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}T${pad(now.getHours())}:${pad(now.getMinutes())}:${pad(now.getSeconds())}`
  const body = `[Trash Info]\nPath=${pathField}\nDeletionDate=${deletion}\nX-Remodeco-Journal=${opts.journalId}\n`
  const fd = fs.openSync(opts.infoPath, 'wx')
  try {
    fs.writeFileSync(fd, body)
    fs.fsyncSync(fd)
  } finally {
    fs.closeSync(fd)
  }
}

export function performTrash(source: string, key: TrashKey, journalId: string, kind: 'home' | 'top' | 'macos'): void {
  if (key.infoPath) {
    const mount = kind === 'top' ? mountPoint(source) : null
    writeTrashInfo({
      infoPath: key.infoPath,
      originalPath: source,
      mount,
      kind: kind === 'macos' ? 'home' : kind,
      journalId,
    })
  }
  renameNoReplace(source, key.filesPath)
}

export function restoreTrash(key: TrashKey): void {
  renameNoReplace(key.filesPath, key.originalPath)
  if (key.infoPath) {
    try {
      fs.unlinkSync(key.infoPath)
    } catch {
      /* ENOENT ok */
    }
  }
}

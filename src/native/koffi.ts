import fs from 'node:fs'
import koffi from 'koffi'

let libc: koffi.IKoffiLib | null = null
let flockFn: koffi.KoffiFunction | null = null
let renameat2Fn: koffi.KoffiFunction | null = null
let renamexNpFn: koffi.KoffiFunction | null = null

const LOCK_EX = 2
const LOCK_NB = 4
const LOCK_UN = 8
const AT_FDCWD = -100
const RENAME_NOREPLACE = 1
const RENAME_EXCL = 0x0004

function loadLibc(): koffi.IKoffiLib {
  if (libc) return libc
  const names =
    process.platform === 'darwin' ? ['libc.dylib'] : ['libc.so.6', 'libc.so']
  let last: unknown
  for (const n of names) {
    try {
      libc = koffi.load(n)
      return libc
    } catch (e) {
      last = e
    }
  }
  throw last instanceof Error ? last : new Error('koffi.load libc failed')
}

function errno(): number {
  try {
    const e = (koffi as unknown as { errno?: () => number }).errno
    if (typeof e === 'function') return e()
  } catch {
    /* ignore */
  }
  return (process as unknown as { errno?: number }).errno ?? 0
}

export function acquireFlock(fd: number): 'ok' | 'busy' {
  if (!flockFn) {
    const lib = loadLibc()
    flockFn = lib.func('flock', 'int', ['int', 'int'])
  }
  for (;;) {
    const rc = flockFn(fd, LOCK_EX | LOCK_NB) as number
    if (rc === 0) return 'ok'
    const err = errno()
    if (err === 4) continue // EINTR
    if (err === 11 || err === 35) return 'busy' // EAGAIN / EWOULDBLOCK (darwin 35)
    throw Object.assign(new Error(`flock failed errno=${err}`), { code: 'FLOCK', errno: err })
  }
}

export function releaseFlock(fd: number): void {
  if (!flockFn) return
  flockFn(fd, LOCK_UN)
}

export function renameNoReplace(from: string, to: string): void {
  if (process.platform === 'linux') {
    if (!renameat2Fn) {
      const lib = loadLibc()
      renameat2Fn = lib.func('renameat2', 'int', ['int', 'str', 'int', 'str', 'uint'])
    }
    const rc = renameat2Fn(AT_FDCWD, from, AT_FDCWD, to, RENAME_NOREPLACE) as number
    if (rc === 0) return
    const err = errno()
    throw posixError(err, from, to)
  }
  if (process.platform === 'darwin') {
    if (!renamexNpFn) {
      const lib = loadLibc()
      renamexNpFn = lib.func('renamex_np', 'int', ['str', 'str', 'uint'])
    }
    const rc = renamexNpFn(from, to, RENAME_EXCL) as number
    if (rc === 0) return
    throw posixError(errno(), from, to)
  }
  throw new Error('renameNoReplace: unsupported platform')
}

function posixError(err: number, from: string, to: string): NodeJS.ErrnoException {
  const map: Record<number, string> = {
    2: 'ENOENT',
    13: 'EACCES',
    17: 'EEXIST',
    18: 'EXDEV',
    21: 'EISDIR',
    39: 'ENOTEMPTY',
  }
  const code = map[err] ?? `ERRNO_${err}`
  const e = new Error(`${code}: ${from} -> ${to}`) as NodeJS.ErrnoException
  e.code = code
  e.errno = err
  return e
}

export function copyNoReplace(from: string, to: string): void {
  fs.copyFileSync(from, to, fs.constants.COPYFILE_EXCL)
}

export function symlinkNoReplace(target: string, dest: string): void {
  fs.symlinkSync(target, dest)
}

export { LOCK_EX, LOCK_NB }

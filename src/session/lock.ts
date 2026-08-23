import fs from 'node:fs'
import path from 'node:path'
import { acquireFlock, releaseFlock } from '../native/koffi.ts'

export type HeldLock = { fd: number; path: string }

export function tryLock(file: string): HeldLock | 'busy' {
  fs.mkdirSync(path.dirname(file), { recursive: true })
  const fd = fs.openSync(file, 'a+')
  const r = acquireFlock(fd)
  if (r === 'busy') {
    fs.closeSync(fd)
    return 'busy'
  }
  fs.ftruncateSync(fd, 0)
  fs.writeSync(fd, JSON.stringify({ pid: process.pid, startedAt: new Date().toISOString() }) + '\n')
  return { fd, path: file }
}

export function unlock(lock: HeldLock): void {
  try {
    releaseFlock(lock.fd)
  } finally {
    fs.closeSync(lock.fd)
  }
}

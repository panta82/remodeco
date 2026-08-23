import fs from 'node:fs'
import path from 'node:path'

export function writeJsonAtomic(file: string, value: unknown): void {
  fs.mkdirSync(path.dirname(file), { recursive: true })
  const tmp = file + '.tmp'
  const json = JSON.stringify(value, null, 2) + '\n'
  const fd = fs.openSync(tmp, 'w')
  try {
    fs.writeFileSync(fd, json)
    fs.fsyncSync(fd)
  } finally {
    fs.closeSync(fd)
  }
  fs.renameSync(tmp, file)
  try {
    const dirFd = fs.openSync(path.dirname(file), 'r')
    try {
      fs.fsyncSync(dirFd)
    } finally {
      fs.closeSync(dirFd)
    }
  } catch {
    throw new Error(`failed to fsync directory for ${file}`)
  }
}

export function writeTextAtomic(file: string, text: string): void {
  fs.mkdirSync(path.dirname(file), { recursive: true })
  const tmp = file + '.tmp'
  const fd = fs.openSync(tmp, 'w')
  try {
    fs.writeFileSync(fd, text)
    fs.fsyncSync(fd)
  } finally {
    fs.closeSync(fd)
  }
  fs.renameSync(tmp, file)
}

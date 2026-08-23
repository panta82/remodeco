import { createHash } from 'node:crypto'

export function sha256Hex(bytes: string | Buffer): string {
  return 'sha256:' + createHash('sha256').update(bytes).digest('hex')
}

export function bodyHash(raw: string): string {
  const start = raw.indexOf('\n---\n')
  const body = start === -1 ? raw : raw.slice(start + 5)
  return sha256Hex(body)
}

export function wholeFileHash(raw: string): string {
  return sha256Hex(raw)
}

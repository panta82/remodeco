import fs from 'node:fs'
import path from 'node:path'
import crypto from 'node:crypto'
import { remodecoPaths } from '../config/paths.ts'
import { writeJsonAtomic, writeTextAtomic } from './atomic.ts'
import type { ManifestFile, SessionRecord } from './types.ts'

export function sessionsRoot(): string {
  return path.join(remodecoPaths().data, 'sessions')
}

export function sessionDir(id: string): string {
  return path.join(sessionsRoot(), id)
}

export function executeLockPath(): string {
  return path.join(remodecoPaths().data, 'execute.lock')
}

export function makeSessionId(root: string): string {
  const d = new Date()
  const pad = (n: number, w = 2) => String(n).padStart(w, '0')
  const ts = `${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}-${pad(d.getHours())}${pad(d.getMinutes())}${pad(d.getSeconds())}`
  const base = path.basename(root).replace(/[^A-Za-z0-9._-]+/g, '_') || 'root'
  const hex = crypto.randomBytes(2).toString('hex')
  return `${ts}-${base}-${hex}`
}

export function readSession(id: string): SessionRecord {
  const p = path.join(sessionDir(id), 'session.json')
  return JSON.parse(fs.readFileSync(p, 'utf8')) as SessionRecord
}

export function writeSession(rec: SessionRecord): void {
  rec.updatedAt = new Date().toISOString()
  writeJsonAtomic(path.join(sessionDir(rec.id), 'session.json'), rec)
}

export function readManifest(id: string): ManifestFile {
  return JSON.parse(fs.readFileSync(path.join(sessionDir(id), 'manifest.json'), 'utf8')) as ManifestFile
}

export function writeManifest(id: string, m: ManifestFile): void {
  writeJsonAtomic(path.join(sessionDir(id), 'manifest.json'), m)
}

export function writePlan(id: string, text: string): string {
  const p = path.join(sessionDir(id), 'plan.md')
  writeTextAtomic(p, text)
  return p
}

export function listSessionIds(): string[] {
  const root = sessionsRoot()
  if (!fs.existsSync(root)) return []
  return fs.readdirSync(root).filter((n) => fs.existsSync(path.join(root, n, 'session.json')))
}

export function listUnfinished(rootPath: string | null): SessionRecord[] {
  const unfinished = new Set([
    'draft',
    'executing',
    'execute-interrupted',
    'undoing',
    'undo-interrupted',
  ])
  const out: SessionRecord[] = []
  for (const id of listSessionIds()) {
    try {
      const s = readSession(id)
      if (!unfinished.has(s.status)) continue
      if (rootPath && s.root !== rootPath) continue
      out.push(s)
    } catch {
      /* skip malformed */
    }
  }
  out.sort((a, b) => (a.updatedAt < b.updatedAt ? 1 : -1))
  return out
}

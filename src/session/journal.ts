import fs from 'node:fs'
import path from 'node:path'
import crypto from 'node:crypto'
import { writeJsonAtomic } from './atomic.ts'
import { sessionDir } from './store.ts'
import type { PlannedStep } from '../exec/schedule.ts'

export type JournalStep = PlannedStep & {
  state: 'pending' | 'in_progress' | 'committed' | 'failed' | 'skipped'
  error?: string
  createdByUs?: boolean
  createdDirFingerprint?: { type: 'directory'; dev: string; ino: string }
  fingerprintTo?: import('../scan/fingerprint.ts').SourceFingerprint
  trashRestoreKey?: { filesPath: string; infoPath: string | null; originalPath: string }
}

export type Journal = {
  journalId: string
  mode: 'execute' | 'undo'
  parentJournalId: string | null
  finalStatus: null | 'executed' | 'undone' | 'aborted' | 'superseded'
  supersededByJournalId: string | null
  expectedPlanHash: string
  expectedRevision: number
  expectedMode: 'move' | 'copy'
  steps: JournalStep[]
}

export function newJournalId(): string {
  return crypto.randomBytes(8).toString('hex')
}

export function journalPath(sessionId: string, journalId: string): string {
  return path.join(sessionDir(sessionId), 'journals', `${journalId}.json`)
}

export function writeJournal(sessionId: string, j: Journal): void {
  writeJsonAtomic(journalPath(sessionId, j.journalId), j)
}

export function readJournal(sessionId: string, journalId: string): Journal {
  return JSON.parse(fs.readFileSync(journalPath(sessionId, journalId), 'utf8')) as Journal
}

export function listJournals(sessionId: string): Journal[] {
  const dir = path.join(sessionDir(sessionId), 'journals')
  if (!fs.existsSync(dir)) return []
  return fs
    .readdirSync(dir)
    .filter((n) => n.endsWith('.json'))
    .map((n) => JSON.parse(fs.readFileSync(path.join(dir, n), 'utf8')) as Journal)
}

export function journalFromSchedule(
  steps: PlannedStep[],
  meta: Pick<Journal, 'expectedPlanHash' | 'expectedRevision' | 'expectedMode'> & {
    mode: 'execute' | 'undo'
    parentJournalId?: string | null
  },
): Journal {
  return {
    journalId: newJournalId(),
    mode: meta.mode,
    parentJournalId: meta.parentJournalId ?? null,
    finalStatus: null,
    supersededByJournalId: null,
    expectedPlanHash: meta.expectedPlanHash,
    expectedRevision: meta.expectedRevision,
    expectedMode: meta.expectedMode,
    steps: steps.map((s) => ({ ...s, state: 'pending' as const })),
  }
}

import fs from 'node:fs'
import path from 'node:path'
import { classifyOps, parsePlan, ParseError } from '../plan/parse.ts'
import { generatePlanMd, idWidthForCount } from '../plan/generate.ts'
import { bodyHash, wholeFileHash } from '../session/hash.ts'
import {
  executeLockPath,
  listUnfinished,
  makeSessionId,
  readManifest,
  readSession,
  sessionDir,
  writeManifest,
  writePlan,
  writeSession,
} from '../session/store.ts'
import { journalFromSchedule, listJournals, writeJournal, type Journal } from '../session/journal.ts'
import { tryLock, unlock, type HeldLock } from '../session/lock.ts'
import { schedule, type PlannedStep } from '../exec/schedule.ts'
import { runJournal } from '../exec/execute.ts'
import { scanTree } from '../scan/walk.ts'
import type { AppConfig } from '../config/load.ts'
import type { ManifestEntry, SessionMode, SessionRecord } from '../session/types.ts'
import { PACKAGE_VERSION } from '../version.ts'

export class ControllerError extends Error {
  constructor(
    readonly exitCode: number,
    message: string,
  ) {
    super(message)
    this.name = 'ControllerError'
  }
}

export function createSession(opts: {
  root: string
  mode: SessionMode
  config: AppConfig
  yes?: boolean
  isTTY?: boolean
}): { session: SessionRecord; sessionLock: HeldLock } {
  const scan = scanTree({
    root: opts.root,
    recursive: opts.config.recursive,
    hidden: opts.config.includeHidden,
    exclude: opts.config.exclude,
  })
  if (scan.rows.length >= 25_000 && !opts.yes) {
    if (!opts.isTTY) {
      throw new ControllerError(4, `scan would include ${scan.rows.length} files; pass --yes to continue`)
    }
  } else if (scan.rows.length >= 5_000) {
    console.error(`warning: ${scan.rows.length} files`)
  }
  const idWidth = idWidthForCount(scan.rows.length)
  const id = makeSessionId(scan.root)
  const entries: ManifestEntry[] = scan.rows.map((r, i) => ({
    ...r,
    id: i + 1,
  }))
  const plan = generatePlanMd({
    sessionId: id,
    root: scan.root,
    rows: entries.map((e) => ({ id: e.id, from: e.from })),
    idWidth,
  })
  const planPath = writePlan(id, plan)
  writeManifest(id, { sessionId: id, root: scan.root, entries })
  const rec: SessionRecord = {
    id,
    root: scan.root,
    mode: opts.mode,
    status: 'draft',
    revision: 1,
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
    planPath,
    manifestPath: path.join(sessionDir(id), 'manifest.json'),
    generatedWholeFileHash: wholeFileHash(plan),
    wholeFileHash: wholeFileHash(plan),
    bodyHash: bodyHash(plan),
    planBodyDiverged: false,
    activeJournalId: null,
    idWidth,
    toolVersion: PACKAGE_VERSION,
    stats: {
      files: scan.rows.filter((r) => r.type === 'file').length,
      symlinks: scan.rows.filter((r) => r.type === 'symlink').length,
      skippedSpecial: scan.skippedSpecial,
      changes: 0,
    },
  }
  writeSession(rec)
  const lock = tryLock(path.join(sessionDir(id), 'session.lock'))
  if (lock === 'busy') throw new ControllerError(3, `session locked: ${id}`)
  return { session: rec, sessionLock: lock }
}

export function openSession(id: string): { session: SessionRecord; sessionLock: HeldLock } {
  const rec = readSession(id)
  const lock = tryLock(path.join(sessionDir(id), 'session.lock'))
  if (lock === 'busy') throw new ControllerError(3, `session locked: ${id}`)
  return { session: rec, sessionLock: lock }
}

export function refreshHashes(session: SessionRecord): SessionRecord {
  const raw = fs.readFileSync(session.planPath, 'utf8')
  const w = wholeFileHash(raw)
  const b = bodyHash(raw)
  if (w !== session.wholeFileHash || b !== session.bodyHash) {
    session.wholeFileHash = w
    session.bodyHash = b
    session.revision += 1
    if (session.status === 'executed' || session.status === 'undone') {
      session.planBodyDiverged = b !== session.bodyHash
    }
    writeSession(session)
  }
  return session
}

export function parseSessionPlan(session: SessionRecord) {
  const raw = fs.readFileSync(session.planPath)
  if (raw.length > 32 * 1024 * 1024) throw new ParseError('plan-too-large', 'plan too large')
  const text = raw.toString('utf8')
  const parsed = parsePlan(text, raw.length)
  if (parsed.unknownLines.length) {
    throw new ParseError('unknown-lines', `unknown lines: ${parsed.unknownLines.map((l) => l.line).join(',')}`)
  }
  const man = readManifest(session.id)
  const map = new Map(man.entries.map((e) => [e.id, e.from]))
  return classifyOps(parsed, map, session.mode)
}

export function dryRun(session: SessionRecord): ReturnType<typeof schedule> {
  const { ops } = parseSessionPlan(session)
  const man = readManifest(session.id)
  return schedule({
    mode: session.mode,
    root: session.root,
    sessionId: session.id,
    manifest: man.entries,
    ops,
  })
}

export function executeSession(session: SessionRecord, expected: {
  revision: number
  mode: SessionMode
  planHash: string
}): Journal {
  if (session.revision !== expected.revision || session.mode !== expected.mode || session.wholeFileHash !== expected.planHash) {
    throw new ControllerError(1, 'session changed since confirm; reload')
  }
  const { ops } = parseSessionPlan(session)
  const man = readManifest(session.id)
  const sched = schedule({
    mode: session.mode,
    root: session.root,
    sessionId: session.id,
    manifest: man.entries,
    ops,
  })
  if (sched.errors.length) throw new ControllerError(1, sched.errors.join('\n'))
  const execLock = tryLock(executeLockPath())
  if (execLock === 'busy') throw new ControllerError(3, 'execute.lock held by another remodeco')
  try {
    const j = journalFromSchedule(sched.steps, {
      mode: 'execute',
      expectedPlanHash: expected.planHash,
      expectedRevision: expected.revision,
      expectedMode: expected.mode,
    })
    writeJournal(session.id, j)
    session.activeJournalId = j.journalId
    session.status = 'executing'
    writeSession(session)
    const done = runJournal(session.id, j)
    if (done.steps.some((s) => s.state === 'failed')) {
      session.status = 'execute-interrupted'
      writeSession(session)
      return done
    }
    session.status = 'executed'
    session.stats.changes = done.steps.filter((s) => s.op !== 'mkdir' && s.state === 'committed').length
    writeSession(session)
    return done
  } finally {
    unlock(execLock)
  }
}

export function undoSession(session: SessionRecord): Journal {
  const journals = listJournals(session.id)
  const active =
    (session.activeJournalId && journals.find((j) => j.journalId === session.activeJournalId)) ||
    journals.find((j) => j.mode === 'execute' && j.finalStatus === 'executed')
  if (!active || active.finalStatus !== 'executed') {
    throw new ControllerError(1, 'no executed journal to undo')
  }
  const inverse: PlannedStep[] = []
  for (const s of [...active.steps].reverse()) {
    if (s.state !== 'committed') continue
    if (s.op === 'commit' || s.op === 'stage') {
      inverse.push({ ...s, stepId: s.stepId + '-u', op: 'commit', from: s.to, to: s.from })
    } else if (s.op === 'copy' && s.to) {
      inverse.push({ ...s, stepId: s.stepId + '-u', op: 'trash', from: s.to, to: undefined })
    } else if (s.op === 'trash' && s.trashRestoreKey) {
      inverse.push({ ...s, stepId: s.stepId + '-u', op: 'commit', from: s.trashRestoreKey.filesPath, to: s.trashRestoreKey.originalPath })
    }
  }
  const execLock = tryLock(executeLockPath())
  if (execLock === 'busy') throw new ControllerError(3, 'execute.lock held')
  try {
    const j = journalFromSchedule(inverse, {
      mode: 'undo',
      parentJournalId: active.journalId,
      expectedPlanHash: session.wholeFileHash,
      expectedRevision: session.revision,
      expectedMode: session.mode,
    })
    writeJournal(session.id, j)
    session.activeJournalId = j.journalId
    session.status = 'undoing'
    writeSession(session)
    const done = runJournal(session.id, j)
    session.status = done.finalStatus === 'undone' ? 'undone' : 'undo-interrupted'
    writeSession(session)
    return done
  } finally {
    unlock(execLock)
  }
}

export function formatSchedule(sched: ReturnType<typeof schedule>): string {
  const lines: string[] = []
  for (const s of sched.steps) {
    if (s.op === 'mkdir') lines.push(`mkdir ${s.mkdirPath}`)
    else if (s.op === 'trash') lines.push(`trash {${s.id}} ${s.from}`)
    else lines.push(`${s.op} {${s.id}} ${s.from} -> ${s.to}`)
  }
  if (sched.trashIds.length) lines.push(`# trash ids: ${sched.trashIds.join(', ')}`)
  if (sched.outsideRoot.length) lines.push(`# outside root: ${sched.outsideRoot.join(', ')}`)
  return lines.join('\n')
}

export { listUnfinished, unlock }

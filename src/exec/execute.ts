import fs from 'node:fs'
import path from 'node:path'
import { copyNoReplace, renameNoReplace, symlinkNoReplace } from '../native/koffi.ts'
import {
  dirIdentityFromStats,
  fingerprintsMatch,
  fingerprintFromStats,
  lstatFingerprint,
} from '../scan/fingerprint.ts'
import type { Journal, JournalStep } from '../session/journal.ts'
import { writeJournal } from '../session/journal.ts'
import { locateTrash, uniqueTrashName, performTrash, restoreTrash } from './trash.ts'

function mkdirOne(p: string): { createdByUs: boolean; identity: ReturnType<typeof dirIdentityFromStats> } {
  try {
    fs.mkdirSync(p, { recursive: false })
    const st = fs.lstatSync(p, { bigint: true })
    return { createdByUs: true, identity: dirIdentityFromStats(st) }
  } catch (e) {
    const err = e as NodeJS.ErrnoException
    if (err.code === 'EEXIST') {
      const st = fs.lstatSync(p, { bigint: true })
      if (st.isDirectory() && !st.isSymbolicLink()) {
        return { createdByUs: false, identity: dirIdentityFromStats(st) }
      }
    }
    throw e
  }
}

export function runJournal(sessionId: string, journal: Journal): Journal {
  for (const step of journal.steps) {
    if (step.state === 'committed' || step.state === 'skipped') continue
    step.state = 'in_progress'
    writeJournal(sessionId, journal)
    try {
      applyStep(sessionId, journal, step)
      if (step.state === 'in_progress') step.state = 'committed'
    } catch (e) {
      const err = e as NodeJS.ErrnoException
      step.state = 'failed'
      step.error = err.code ? `${err.code}: ${err.message}` : String(e)
      writeJournal(sessionId, journal)
      journal.finalStatus = null
      return journal
    }
    writeJournal(sessionId, journal)
  }
  journal.finalStatus = journal.mode === 'undo' ? 'undone' : 'executed'
  writeJournal(sessionId, journal)
  return journal
}

function applyStep(sessionId: string, journal: Journal, step: JournalStep): void {
  switch (step.op) {
    case 'mkdir': {
      if (!step.mkdirPath) throw new Error('mkdir without path')
      const r = mkdirOne(step.mkdirPath)
      step.createdByUs = r.createdByUs
      step.createdDirFingerprint = r.identity
      return
    }
    case 'stage':
    case 'commit':
    case 'case-stage': {
      if (!step.from || !step.to) throw new Error('rename without paths')
      renameNoReplace(step.from, step.to)
      const st = fs.lstatSync(step.to, { bigint: true })
      const t = st.isSymbolicLink() ? 'symlink' : 'file'
      step.fingerprintTo = fingerprintFromStats(st, t)
      return
    }
    case 'copy': {
      if (!step.from || !step.to) throw new Error('copy without paths')
      const stFrom = fs.lstatSync(step.from, { bigint: true })
      if (stFrom.isSymbolicLink()) {
        const target = fs.readlinkSync(step.from)
        symlinkNoReplace(target, step.to)
      } else {
        copyNoReplace(step.from, step.to)
      }
      const st = fs.lstatSync(step.to, { bigint: true })
      step.fingerprintTo = fingerprintFromStats(st, st.isSymbolicLink() ? 'symlink' : 'file')
      return
    }
    case 'trash': {
      if (!step.from || !step.fingerprintFrom) throw new Error('trash without from')
      const live = lstatFingerprint(step.from, step.fingerprintFrom.type)
      if (!fingerprintsMatch(live, step.fingerprintFrom)) {
        throw Object.assign(new Error('fingerprint mismatch'), { code: 'ESTALE' })
      }
      const loc = locateTrash(step.from)
      const base = path.basename(step.from)
      const uniq = uniqueTrashName(loc.root, base, loc.kind === 'macos')
      step.trashRestoreKey = {
        filesPath: uniq.filesPath,
        infoPath: uniq.infoPath,
        originalPath: step.from,
      }
      writeJournal(sessionId, journal)
      performTrash(step.from, step.trashRestoreKey, journal.journalId, loc.kind)
      const st = fs.lstatSync(step.trashRestoreKey.filesPath, { bigint: true })
      step.fingerprintTo = fingerprintFromStats(st, st.isSymbolicLink() ? 'symlink' : 'file')
      return
    }
    default:
      throw new Error(`unknown op ${step.op}`)
  }
}

export function undoTrashStep(step: JournalStep): void {
  if (!step.trashRestoreKey) throw new Error('no trash key')
  restoreTrash(step.trashRestoreKey)
}

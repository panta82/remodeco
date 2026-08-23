import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { describe, expect, it } from 'vitest'
import { scanTree } from '../src/scan/walk.ts'
import { schedule } from '../src/exec/schedule.ts'
import { journalFromSchedule } from '../src/session/journal.ts'
import { runJournal } from '../src/exec/execute.ts'
import { writeJournal } from '../src/session/journal.ts'
import { sessionDir } from '../src/session/store.ts'

describe('execute rename', () => {
  it('moves a file with mkdir', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'remodeco-ex-'))
    process.env.REMODECO_DATA_DIR = path.join(dir, 'data')
    process.env.REMODECO_CONFIG_DIR = path.join(dir, 'cfg')
    fs.writeFileSync(path.join(dir, 'a.txt'), 'hello')
    const scan = scanTree({ root: dir, recursive: true, hidden: false, exclude: [] })
    const man = scan.rows.map((r, i) => ({ ...r, id: i + 1 }))
    const dest = path.join(dir, 'nested', 'b.txt')
    const sched = schedule({
      mode: 'move',
      root: scan.root,
      sessionId: 'sess1',
      manifest: man,
      ops: [{ id: 1, from: man[0].from, to: dest, kind: 'move' }],
    })
    expect(sched.errors).toEqual([])
    const j = journalFromSchedule(sched.steps, {
      mode: 'execute',
      expectedPlanHash: 'sha256:x',
      expectedRevision: 1,
      expectedMode: 'move',
    })
    fs.mkdirSync(path.join(sessionDir('sess1'), 'journals'), { recursive: true })
    writeJournal('sess1', j)
    const done = runJournal('sess1', j)
    expect(done.finalStatus).toBe('executed')
    expect(fs.readFileSync(dest, 'utf8')).toBe('hello')
    expect(fs.existsSync(path.join(dir, 'a.txt'))).toBe(false)
  })
})

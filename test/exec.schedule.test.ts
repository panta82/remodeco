import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { describe, expect, it } from 'vitest'
import { scanTree } from '../src/scan/walk.ts'
import { schedule } from '../src/exec/schedule.ts'
import { lstatFingerprint } from '../src/scan/fingerprint.ts'

describe('schedule', () => {
  it('noops produce no file steps', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'remodeco-sch-'))
    fs.writeFileSync(path.join(dir, 'a.txt'), 'a')
    const scan = scanTree({ root: dir, recursive: true, hidden: false, exclude: [] })
    const man = scan.rows.map((r, i) => ({ ...r, id: i + 1 }))
    const sched = schedule({
      mode: 'move',
      root: scan.root,
      sessionId: 's',
      manifest: man,
      ops: man.map((m) => ({ id: m.id, from: m.from, to: m.from, kind: 'noop' as const })),
    })
    expect(sched.steps.filter((s) => s.op === 'commit')).toHaveLength(0)
  })

  it('rename into new folder schedules mkdir then commit', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'remodeco-sch-'))
    fs.writeFileSync(path.join(dir, 'a.txt'), 'a')
    const scan = scanTree({ root: dir, recursive: true, hidden: false, exclude: [] })
    const man = scan.rows.map((r, i) => ({ ...r, id: i + 1, ...lstatFingerprint(r.from, 'file') }))
    const dest = path.join(dir, 'sub', 'b.txt')
    const sched = schedule({
      mode: 'move',
      root: scan.root,
      sessionId: 's',
      manifest: man,
      ops: [{ id: 1, from: man[0].from, to: dest, kind: 'move' }],
    })
    expect(sched.errors).toEqual([])
    expect(sched.steps.some((s) => s.op === 'mkdir')).toBe(true)
    expect(sched.steps.some((s) => s.op === 'commit')).toBe(true)
  })
})

import fs from 'node:fs'
import path from 'node:path'
import crypto from 'node:crypto'
import type { ManifestEntry } from '../session/types.ts'
import { resolveDestParent, destOutsideRoot, type DestParentRef } from './dest.ts'
import { fingerprintsMatch, lstatFingerprint, type SourceFingerprint } from '../scan/fingerprint.ts'

export type PlannedKind = 'mkdir' | 'stage' | 'commit' | 'copy' | 'trash' | 'noop'

export type PlannedStep = {
  stepId: string
  op: PlannedKind
  from?: string
  to?: string
  id?: number
  fingerprintFrom?: SourceFingerprint
  destParentRef?: DestParentRef
  mkdirPath?: string
}

export type ScheduleResult = {
  steps: PlannedStep[]
  outsideRoot: string[]
  trashIds: number[]
  errors: string[]
}

function sid(): string {
  return crypto.randomBytes(6).toString('hex')
}

export function schedule(opts: {
  mode: 'move' | 'copy'
  root: string
  sessionId: string
  manifest: ManifestEntry[]
  ops: { id: number; from: string; to: string; kind: 'skip' | 'noop' | 'trash' | 'move' | 'copy' }[]
}): ScheduleResult {
  const byId = new Map(opts.manifest.map((m) => [m.id, m]))
  const errors: string[] = []
  const outsideRoot: string[] = []
  const trashIds: number[] = []
  const steps: PlannedStep[] = []
  const mkdirMade = new Map<string, string>() // path -> stepId

  const fromsMoving = new Set(
    opts.ops.filter((o) => o.kind === 'move' || o.kind === 'copy').map((o) => o.from),
  )
  const dests = opts.ops.filter((o) => o.kind === 'move' || o.kind === 'copy').map((o) => o.to)

  // occupancy among dests
  const destCount = new Map<string, number>()
  for (const d of dests) destCount.set(d, (destCount.get(d) ?? 0) + 1)
  for (const [d, n] of destCount) {
    if (n > 1) errors.push(`duplicate dest ${d}`)
  }

  const remaining = opts.ops.filter((o) => o.kind === 'move' || o.kind === 'copy')
  const committedFrom = new Set<string>()
  const maxIter = remaining.length * remaining.length + 4
  let iter = 0
  const staged = new Set<number>()

  function ensureMkdirs(to: string): DestParentRef {
    const resolved = resolveDestParent(to)
    let parent = resolved.parentPath
    let prevRef: DestParentRef = {
      kind: 'existing',
      identity: resolved.identity,
      parentPath: parent,
      suffix: resolved.suffix,
    }
    const dirsToCreate = resolved.suffix.slice(0, -1) // all but leaf
    for (const seg of dirsToCreate) {
      const next = parent === '/' ? `/${seg}` : `${parent}/${seg}`
      const existing = mkdirMade.get(next)
      if (existing) {
        prevRef = { kind: 'mkdirStep', stepId: existing, parentPath: next, suffix: [] }
        parent = next
        continue
      }
      try {
        const st = fs.lstatSync(next, { bigint: true })
        if (st.isDirectory() && !st.isSymbolicLink()) {
          parent = next
          continue
        }
      } catch {
        /* create */
      }
      const stepId = sid()
      steps.push({
        stepId,
        op: 'mkdir',
        mkdirPath: next,
        destParentRef: prevRef,
      })
      mkdirMade.set(next, stepId)
      prevRef = { kind: 'mkdirStep', stepId, parentPath: next, suffix: [] }
      parent = next
    }
    const leafSuffix = resolved.suffix.slice(dirsToCreate.length)
    return { ...prevRef, suffix: leafSuffix.length ? leafSuffix : [path.posix.basename(to)] }
  }

  while (remaining.length && iter++ < maxIter) {
    let progress = false
    for (let i = 0; i < remaining.length; i++) {
      const op = remaining[i]
      const entry = byId.get(op.id)
      if (!entry) {
        errors.push(`missing manifest ${op.id}`)
        remaining.splice(i, 1)
        i--
        continue
      }
      try {
        const live = lstatFingerprint(op.from, entry.type)
        if (!fingerprintsMatch(live, entry)) {
          errors.push(`fingerprint mismatch {${op.id}} ${op.from}`)
          remaining.splice(i, 1)
          i--
          continue
        }
      } catch {
        errors.push(`missing source {${op.id}} ${op.from}`)
        remaining.splice(i, 1)
        i--
        continue
      }

      if (destOutsideRoot(opts.root, op.to)) outsideRoot.push(op.to)
      const parentRef = ensureMkdirs(op.to)

      let destExists = false
      try {
        fs.lstatSync(op.to)
        destExists = true
      } catch {
        destExists = false
      }

      if (destExists && op.to !== op.from) {
        if (opts.mode === 'copy') {
          errors.push(`copy dest exists {${op.id}} ${op.to}`)
          remaining.splice(i, 1)
          i--
          continue
        }
        if (fromsMoving.has(op.to) && !committedFrom.has(op.to)) {
          // need vacate first; if we're in a cycle, stage
          continue
        }
        errors.push(`dest exists {${op.id}} ${op.to}`)
        remaining.splice(i, 1)
        i--
        continue
      }

      const stepId = sid()
      steps.push({
        stepId,
        op: opts.mode === 'copy' ? 'copy' : 'commit',
        from: op.from,
        to: op.to,
        id: op.id,
        fingerprintFrom: entry,
        destParentRef: parentRef,
      })
      committedFrom.add(op.from)
      remaining.splice(i, 1)
      i--
      progress = true
    }
    if (!progress && remaining.length) {
      // cycle: stage first remaining
      const op = remaining[0]
      if (staged.has(op.id)) {
        errors.push(`unresolvable dest cycle involving {${op.id}}`)
        break
      }
      staged.add(op.id)
      const tmp = path.join(
        path.dirname(op.from),
        `.remodeco-stage-${opts.sessionId}-${sid()}`,
      )
      const parentRef = ensureMkdirs(tmp)
      const entry = byId.get(op.id)!
      steps.push({
        stepId: sid(),
        op: 'stage',
        from: op.from,
        to: tmp,
        id: op.id,
        fingerprintFrom: entry,
        destParentRef: parentRef,
      })
      committedFrom.add(op.from)
      remaining[0] = { ...op, from: tmp }
      fromsMoving.delete(op.from)
      fromsMoving.add(tmp)
    }
  }

  for (const op of opts.ops) {
    if (op.kind !== 'trash') continue
    trashIds.push(op.id)
    const entry = byId.get(op.id)
    if (!entry) {
      errors.push(`missing manifest ${op.id}`)
      continue
    }
    steps.push({
      stepId: sid(),
      op: 'trash',
      from: op.from,
      id: op.id,
      fingerprintFrom: entry,
    })
  }

  return { steps, outsideRoot, trashIds, errors }
}

import type { SourceFingerprint } from '../scan/fingerprint.ts'

export type SessionMode = 'move' | 'copy'
export type SessionStatus =
  | 'draft'
  | 'executing'
  | 'execute-interrupted'
  | 'executed'
  | 'undoing'
  | 'undo-interrupted'
  | 'undone'

export type SessionRecord = {
  id: string
  root: string
  mode: SessionMode
  status: SessionStatus
  revision: number
  createdAt: string
  updatedAt: string
  planPath: string
  manifestPath: string
  generatedWholeFileHash: string
  wholeFileHash: string
  bodyHash: string
  planBodyDiverged: boolean
  activeJournalId: string | null
  idWidth: number
  toolVersion: string
  stats: {
    files: number
    symlinks: number
    skippedSpecial: number
    changes: number
  }
}

export type ManifestEntry = SourceFingerprint & {
  id: number
  from: string
}

export type ManifestFile = {
  sessionId: string
  root: string
  entries: ManifestEntry[]
}

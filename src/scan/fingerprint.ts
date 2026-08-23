import fs from 'node:fs'

export type SourceFingerprint = {
  type: 'file' | 'symlink'
  dev: string
  ino: string
  size: string
  nlink: string
  mtimeNs: string
  mode: number
}

export type DirectoryIdentity = {
  type: 'directory'
  dev: string
  ino: string
}

export function fingerprintsMatch(a: SourceFingerprint, b: SourceFingerprint): boolean {
  return (
    a.type === b.type &&
    a.dev === b.dev &&
    a.ino === b.ino &&
    a.size === b.size &&
    a.nlink === b.nlink &&
    a.mtimeNs === b.mtimeNs
  )
}

export function fingerprintFromStats(
  st: fs.BigIntStats,
  type: 'file' | 'symlink',
): SourceFingerprint {
  return {
    type,
    dev: st.dev.toString(),
    ino: st.ino.toString(),
    size: st.size.toString(),
    nlink: st.nlink.toString(),
    mtimeNs: (st.mtimeNs ?? st.mtimeMs * 1_000_000n).toString(),
    mode: Number(st.mode),
  }
}

export function dirIdentityFromStats(st: fs.BigIntStats): DirectoryIdentity {
  return { type: 'directory', dev: st.dev.toString(), ino: st.ino.toString() }
}

export function lstatFingerprint(absPath: string, type: 'file' | 'symlink'): SourceFingerprint {
  const st = fs.lstatSync(absPath, { bigint: true })
  return fingerprintFromStats(st, type)
}

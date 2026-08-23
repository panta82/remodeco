export type OpKind = 'skip' | 'noop' | 'trash' | 'move' | 'copy'

export type ParsedBullet = {
  id: number
  destRaw: string
  kind: 'trash' | 'dest'
  line: number
}

export type PlanHeader = {
  remodeco: number
  id: string
  root: string
}

export type ParsedPlan = {
  header: PlanHeader | null
  bullets: ParsedBullet[]
  skippedIds: number[]
  unknownLines: { line: number; text: string }[]
}

export class ParseError extends Error {
  constructor(
    readonly code: string,
    message?: string,
  ) {
    super(message ?? code)
    this.name = 'ParseError'
  }
}

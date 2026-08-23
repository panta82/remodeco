import { describe, expect, it } from 'vitest'
import { isHeadlessRemote, resolveEditor } from '../src/tui/editor.ts'
import type { AppConfig } from '../src/config/load.ts'

const base: AppConfig = {
  editor: undefined,
  editorMode: 'auto',
  exclude: [],
  includeHidden: false,
  recursive: true,
  openEditor: true,
}

describe('isHeadlessRemote', () => {
  it('detects ssh', () => {
    expect(isHeadlessRemote({ SSH_CONNECTION: '1.2.3.4 1 5.6.7.8 22' })).toBe(true)
  })
  it('does not treat vscode remote as headless', () => {
    expect(isHeadlessRemote({ SSH_CONNECTION: 'x', TERM_PROGRAM: 'vscode' })).toBe(false)
  })
})

describe('resolveEditor', () => {
  it('prefers EDITOR over gui on ssh', () => {
    const ed = resolveEditor(base, { SSH_CONNECTION: 'x', EDITOR: 'nvim', PATH: process.env.PATH })
    expect(ed?.argv[0]).toBe('nvim')
    expect(ed?.kind).toBe('tty')
  })
})

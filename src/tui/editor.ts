import { spawn } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import type { AppConfig } from '../config/load.ts'

const GUI = new Set(['code', 'codium', 'subl', 'kate', 'gedit', 'code-insiders'])
const TTY_EDITORS = new Set(['vi', 'vim', 'nvim', 'nano', 'pico', 'emacs', 'hx', 'helix', 'micro', 'ed'])

export function tokenizeEditor(cmd: string): string[] {
  const out: string[] = []
  let cur = ''
  let q: '"' | "'" | null = null
  for (let i = 0; i < cmd.length; i++) {
    const c = cmd[i]
    if (q) {
      if (c === q) q = null
      else cur += c
      continue
    }
    if (c === '"' || c === "'") {
      q = c
      continue
    }
    if (c === ' ' || c === '\t') {
      if (cur) out.push(cur)
      cur = ''
      continue
    }
    cur += c
  }
  if (cur) out.push(cur)
  return out
}

/** SSH login without VS Code Remote — do not auto-launch GUI editors. */
export function isHeadlessRemote(env: NodeJS.ProcessEnv = process.env): boolean {
  const ssh = Boolean(env.SSH_CONNECTION || env.SSH_CLIENT || env.SSH_TTY)
  if (!ssh) return false
  if (env.TERM_PROGRAM === 'vscode') return false
  return true
}

export function resolveEditor(
  config: AppConfig,
  env: NodeJS.ProcessEnv = process.env,
): { argv: string[]; kind: 'gui' | 'tty' } | null {
  let argv: string[] | undefined
  let explicit = false
  if (Array.isArray(config.editor)) {
    argv = config.editor
    explicit = true
  } else if (typeof config.editor === 'string') {
    argv = tokenizeEditor(config.editor)
    explicit = true
  }
  if (!argv?.length && env.VISUAL) {
    argv = tokenizeEditor(env.VISUAL)
  }
  if (!argv?.length && env.EDITOR) {
    argv = tokenizeEditor(env.EDITOR)
  }
  const allowGuiAuto = config.editorMode === 'gui' || (!isHeadlessRemote(env) && config.editorMode !== 'tty')
  if (!argv?.length && allowGuiAuto) {
    for (const name of ['code', 'codium', 'subl', 'kate', 'gedit']) {
      if (which(name)) {
        argv = [name]
        break
      }
    }
  }
  if (!argv?.length) {
    if (which('vi')) argv = ['vi']
    else if (which('nano')) argv = ['nano']
  }
  if (!argv?.length) return null
  const base = path.basename(argv[0])
  let kind: 'gui' | 'tty' = GUI.has(base) ? 'gui' : TTY_EDITORS.has(base) ? 'tty' : 'tty'
  if (config.editorMode === 'gui') kind = 'gui'
  if (config.editorMode === 'tty') kind = 'tty'
  if (isHeadlessRemote(env) && kind === 'gui' && !explicit && config.editorMode === 'auto') {
    kind = 'tty'
  }
  return { argv, kind }
}

function which(cmd: string): boolean {
  const paths = (process.env.PATH ?? '').split(path.delimiter)
  return paths.some((p) => {
    try {
      fs.accessSync(path.join(p, cmd), fs.constants.X_OK)
      return true
    } catch {
      return false
    }
  })
}

function multiplexer(): { type: 'tmux' | 'zellij' | 'kitty' | 'wezterm' } | null {
  if (process.env.TMUX) return { type: 'tmux' }
  if (process.env.ZELLIJ) return { type: 'zellij' }
  if (process.env.KITTY_WINDOW_ID) return { type: 'kitty' }
  if (process.env.WEZTERM_PANE) return { type: 'wezterm' }
  return null
}

function spawnMux(mux: NonNullable<ReturnType<typeof multiplexer>>, argv: string[], planPath: string): boolean {
  try {
    if (mux.type === 'tmux') {
      spawn('tmux', ['split-window', '-h', ...argv, planPath], { stdio: 'ignore' }).unref()
      return true
    }
    if (mux.type === 'zellij') {
      spawn('zellij', ['action', 'new-pane', '--', ...argv, planPath], { stdio: 'ignore' }).unref()
      return true
    }
    if (mux.type === 'kitty') {
      spawn('kitty', ['@', 'launch', '--type=tab', ...argv, planPath], { stdio: 'ignore' }).unref()
      return true
    }
    if (mux.type === 'wezterm') {
      spawn('wezterm', ['cli', 'spawn', '--new-tab', ...argv, planPath], { stdio: 'ignore' }).unref()
      return true
    }
  } catch {
    return false
  }
  return false
}

export async function launchEditor(config: AppConfig, planPath: string): Promise<void> {
  if (!config.openEditor) {
    console.error(`plan: ${planPath}`)
    return
  }
  const ed = resolveEditor(config)
  if (!ed) {
    console.error(`plan: ${planPath}`)
    return
  }
  if (ed.kind === 'gui') {
    spawn(ed.argv[0], [...ed.argv.slice(1), planPath], { stdio: 'ignore', detached: true }).unref()
    return
  }
  const mux = multiplexer()
  if (mux && spawnMux(mux, ed.argv, planPath)) return
  await attachAndWait(ed.argv, planPath)
}

export function attachAndWait(argv: string[], planPath: string): Promise<number> {
  return new Promise((resolve) => {
    const child = spawn(argv[0], [...argv.slice(1), planPath], {
      stdio: 'inherit',
    })
    const ign = () => {}
    process.on('SIGINT', ign)
    child.on('exit', (code) => {
      process.off('SIGINT', ign)
      resolve(code ?? 0)
    })
    child.on('error', () => {
      process.off('SIGINT', ign)
      resolve(1)
    })
  })
}

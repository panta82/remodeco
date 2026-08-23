import fs from 'node:fs'
import path from 'node:path'
import { DEFAULT_EXCLUDES } from '../scan/walk.ts'
import { remodecoPaths } from './paths.ts'
import type { CliArgs } from '../cli/args.ts'

export type AppConfig = {
  editor: string | string[] | undefined
  editorMode: 'auto' | 'gui' | 'tty'
  exclude: string[]
  includeHidden: boolean
  recursive: boolean
  openEditor: boolean
}

const DEFAULTS: AppConfig = {
  editor: undefined,
  editorMode: 'auto',
  exclude: [...DEFAULT_EXCLUDES],
  includeHidden: false,
  recursive: true,
  openEditor: true,
}

function readJson(file: string): Partial<AppConfig> {
  try {
    const raw = fs.readFileSync(file, 'utf8')
    return JSON.parse(raw) as Partial<AppConfig>
  } catch {
    return {}
  }
}

function merge(a: AppConfig, b: Partial<AppConfig>, appendExclude: boolean): AppConfig {
  const out = { ...a, ...b }
  if (appendExclude && b.exclude) {
    out.exclude = [...a.exclude, ...b.exclude]
  }
  return out
}

export function loadConfig(scanRoot: string, args: CliArgs): AppConfig {
  const paths = remodecoPaths()
  let cfg = { ...DEFAULTS }
  cfg = merge(cfg, readJson(path.join(paths.config, 'config.json')), true)
  cfg = merge(cfg, readJson(path.join(scanRoot, '.remodeco.json')), true)
  if (args.noDefaultExcludes) cfg.exclude = []
  if (args.exclude.length) cfg.exclude = [...cfg.exclude, ...args.exclude]
  if (args.hidden) cfg.includeHidden = true
  if (args.noRecursive) cfg.recursive = false
  if (args.editor) cfg.editor = args.editor
  if (args.editorMode) cfg.editorMode = args.editorMode
  if (args.noEdit || process.env.REMODECO_NO_EDIT === '1') cfg.openEditor = false
  if (process.env.REMODECO_EDITOR) cfg.editor = process.env.REMODECO_EDITOR
  return cfg
}

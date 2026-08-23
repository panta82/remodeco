import { parseArgs } from 'node:util'

export type CliArgs = {
  dir: string | undefined
  copy: boolean
  noEdit: boolean
  noRecursive: boolean
  hidden: boolean
  exclude: string[]
  noDefaultExcludes: boolean
  editor: string | undefined
  editorMode: 'auto' | 'gui' | 'tty' | undefined
  newSession: boolean
  session: string | undefined
  listSessions: boolean
  dryRun: boolean
  yes: boolean
  verbose: boolean
  help: boolean
  version: boolean
  undo: string | undefined
}

export function parseCli(argv: string[]): CliArgs {
  const { values, positionals } = parseArgs({
    args: argv.slice(2),
    allowPositionals: true,
    strict: true,
    options: {
      copy: { type: 'boolean', default: false },
      'no-edit': { type: 'boolean', default: false },
      'no-recursive': { type: 'boolean', default: false },
      hidden: { type: 'boolean', default: false },
      exclude: { type: 'string', multiple: true },
      'no-default-excludes': { type: 'boolean', default: false },
      editor: { type: 'string' },
      'editor-mode': { type: 'string' },
      new: { type: 'boolean', default: false },
      session: { type: 'string' },
      'list-sessions': { type: 'boolean', default: false },
      'dry-run': { type: 'boolean', default: false },
      yes: { type: 'boolean', default: false },
      verbose: { type: 'boolean', default: false },
      help: { type: 'boolean', short: 'h', default: false },
      version: { type: 'boolean', short: 'v', default: false },
    },
  })

  let undo: string | undefined
  let dir: string | undefined
  if (positionals[0] === 'undo') {
    undo = positionals[1]
    if (!undo) throw new UsageError('remodeco undo <session-id>')
  } else if (positionals[0]) {
    dir = positionals[0]
  }

  const editorMode = values['editor-mode']
  if (editorMode && editorMode !== 'auto' && editorMode !== 'gui' && editorMode !== 'tty') {
    throw new UsageError('--editor-mode must be auto, gui, or tty')
  }

  return {
    dir,
    copy: values.copy === true,
    noEdit: values['no-edit'] === true,
    noRecursive: values['no-recursive'] === true,
    hidden: values.hidden === true,
    exclude: values.exclude ?? [],
    noDefaultExcludes: values['no-default-excludes'] === true,
    editor: values.editor,
    editorMode: editorMode as CliArgs['editorMode'],
    newSession: values.new === true,
    session: values.session,
    listSessions: values['list-sessions'] === true,
    dryRun: values['dry-run'] === true,
    yes: values.yes === true,
    verbose: values.verbose === true,
    help: values.help === true,
    version: values.version === true,
    undo,
  }
}

export class UsageError extends Error {}

export const HELP = `Usage: remodeco [dir] [options]
       remodeco undo <session-id>
       remodeco --list-sessions

Review and execute file renames (or copies) from an editor, with an Ink TUI.

  --copy                 session is copy, not move
  --no-edit              do not launch an editor; TUI only
  --no-recursive         do not descend
  --hidden               include hidden files
  --exclude <pattern>    extra exclude (repeatable)
  --no-default-excludes  do not skip .git / node_modules / .hg / .svn
  --editor <cmd>         override editor
  --editor-mode <mode>   auto | gui | tty
  --new                  never resume; always new session
  --session <id>         open an existing session
  --list-sessions        print sessions and exit
  --dry-run              validate + print schedule; no mutations
  --yes                  skip large-scan prompt only
  --verbose              debug to stderr
  -h, --help
  -v, --version
`

import fs from 'node:fs'
import path from 'node:path'
import { HELP, parseCli, UsageError } from './args.ts'
import { requireSupportedPlatform } from '../platform.ts'
import { setVerbose } from '../log.ts'
import { PACKAGE_VERSION } from '../version.ts'

async function main(): Promise<void> {
  let args
  try {
    args = parseCli(process.argv)
  } catch (e) {
    if (e instanceof UsageError) {
      console.error(e.message)
      process.exit(2)
    }
    throw e
  }
  if (args.help) {
    process.stdout.write(HELP)
    return
  }
  if (args.version) {
    process.stdout.write(PACKAGE_VERSION + '\n')
    return
  }

  requireSupportedPlatform()
  setVerbose(args.verbose)

  const {
    ControllerError,
    createSession,
    dryRun,
    executeSession,
    undoSession,
    formatSchedule,
    listUnfinished,
    openSession,
    parseSessionPlan,
    refreshHashes,
    unlock,
  } = await import('../controller/sessionController.ts')
  const { listSessionIds, readSession } = await import('../session/store.ts')
  const { loadConfig } = await import('../config/load.ts')
  const { launchEditor } = await import('../tui/editor.ts')

  if (args.listSessions) {
    for (const id of listSessionIds()) {
      try {
        const s = readSession(id)
        console.log(`${s.id}\t${s.status}\t${s.mode}\t${s.root}`)
      } catch {
        console.log(`${id}\t(unreadable)`)
      }
    }
    return
  }

  if (args.undo) {
    const opened = openSession(args.undo)
    try {
      const j = undoSession(opened.session)
      console.log(j.finalStatus === 'undone' ? `undone ${j.journalId}` : `undo interrupted ${j.journalId}`)
      process.exit(j.finalStatus === 'undone' ? 0 : 1)
    } finally {
      unlock(opened.sessionLock)
    }
  }

  const isTTY = Boolean(process.stdin.isTTY && process.stdout.isTTY)
  let session
  let sessionLock
  try {
    if (args.session) {
      ;({ session, sessionLock } = openSession(args.session))
      if (args.dir) {
        const want = fs.realpathSync(args.dir)
        if (want !== session.root) {
          console.error(`warning: --session root ${session.root} != ${want}; using session root`)
        }
      }
      if (args.copy && session.mode !== 'copy') {
        if (!isTTY) {
          console.error('--copy does not match stored session mode')
          process.exit(2)
        }
        session.mode = 'copy'
        session.revision += 1
      }
    } else {
      const dir = args.dir ? path.resolve(args.dir) : process.cwd()
      const root = fs.realpathSync(dir)
      const unfinished = args.newSession ? [] : listUnfinished(root)
      if (unfinished.length && !args.newSession) {
        const pick = unfinished[0]
        if (unfinished.length > 1) {
          for (const u of unfinished.slice(1)) {
            console.error(
              `also unfinished: ${u.id} (${u.status}, updated ${u.updatedAt}) — --session ${u.id} to open`,
            )
          }
        }
        console.error(`resuming ${pick.id}`)
        ;({ session, sessionLock } = openSession(pick.id))
        if (args.copy && session.mode !== 'copy' && !isTTY) {
          console.error('--copy does not match stored session mode')
          process.exit(2)
        }
        if (args.copy) {
          session.mode = 'copy'
          session.revision += 1
        }
      } else {
        const config = loadConfig(root, args)
        ;({ session, sessionLock } = createSession({
          root,
          mode: args.copy ? 'copy' : 'move',
          config,
          yes: args.yes,
          isTTY,
        }))
      }
    }
  } catch (e) {
    if (e instanceof ControllerError) {
      console.error(e.message)
      process.exit(e.exitCode)
    }
    throw e
  }

  const config = loadConfig(session.root, args)

  try {
    refreshHashes(session)
    if (config.openEditor) {
      await launchEditor(config, session.planPath)
    } else {
      console.error(`plan: ${session.planPath}`)
    }

    if (args.dryRun) {
      const sched = dryRun(session)
      if (sched.errors.length) {
        console.error(sched.errors.join('\n'))
        process.exit(1)
      }
      console.log(formatSchedule(sched))
      return
    }

    if (!isTTY) {
      console.error(`plan: ${session.planPath}`)
      console.error('non-TTY: use --dry-run, or run on a TTY for Execute')
      process.exit(4)
    }

    const { runTui } = await import(new URL('./tui/app.js', import.meta.url).href)
    await runTui({
      session,
      config,
      onExecute: () => {
        const s = refreshHashes(session)
        parseSessionPlan(s)
        return executeSession(s, {
          revision: s.revision,
          mode: s.mode,
          planHash: s.wholeFileHash,
        })
      },
    })
  } finally {
    unlock(sessionLock)
  }
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})

import React, { useEffect, useMemo, useState } from 'react'
import { Box, Text, useApp, useInput, render } from 'ink'
import chokidar from 'chokidar'
import type { SessionRecord } from '../session/types.ts'
import type { AppConfig } from '../config/load.ts'
import { parseSessionPlan, refreshHashes } from '../controller/sessionController.ts'
import { diffWords } from '../diff/diff.ts'
import { attachAndWait, launchEditor, resolveEditor } from './editor.ts'
import type { Journal } from '../session/journal.ts'

export function App(props: {
  session: SessionRecord
  config: AppConfig
  onExecute: () => Journal
}): React.JSX.Element {
  const { exit } = useApp()
  const [session, setSession] = useState(props.session)
  const [filter, setFilter] = useState('')
  const [status, setStatus] = useState('')
  const [confirmTrash, setConfirmTrash] = useState<number[] | null>(null)

  const ops = useMemo(() => {
    try {
      return parseSessionPlan(session).ops
    } catch (e) {
      return []
    }
  }, [session.wholeFileHash, session.mode, session.revision])

  useEffect(() => {
    const w = chokidar.watch(session.planPath, {
      ignoreInitial: true,
      awaitWriteFinish: { stabilityThreshold: 200, pollInterval: 50 },
    })
    w.on('change', () => {
      setSession({ ...refreshHashes(session) })
    })
    return () => {
      void w.close()
    }
  }, [session.planPath])

  const shown = ops.filter((o) => {
    if (!filter) return true
    const q = filter.toLowerCase()
    return o.from.toLowerCase().includes(q) || o.to.toLowerCase().includes(q) || String(o.id).includes(q)
  })
  const changes = ops.filter((o) => o.kind !== 'noop' && o.kind !== 'skip')
  const trash = ops.filter((o) => o.kind === 'trash')

  useInput((input, key) => {
    if (confirmTrash) {
      if (input === 'y' || input === 'Y') {
        setConfirmTrash(null)
        runExec()
      } else {
        setConfirmTrash(null)
        setStatus('cancelled confirm')
      }
      return
    }
    if (key.escape || input === 'c') {
      exit()
      return
    }
    if (input === 'e') {
      if (trash.length) {
        setConfirmTrash(trash.map((t) => t.id))
        return
      }
      runExec()
      return
    }
    if (input === 'o') {
      const ed = resolveEditor(props.config)
      if (!ed) return
      void (async () => {
        await attachAndWait(ed.argv, session.planPath)
        setSession({ ...refreshHashes(session) })
      })()
      return
    }
    if (input === '/' ) {
      /* filter not interactive in v1 mini */
    }
  })

  function runExec() {
    try {
      const j = props.onExecute()
      if (j.finalStatus === 'executed') {
        setStatus(`executed ${j.journalId}`)
        setTimeout(() => exit(), 200)
      } else {
        setStatus(`interrupted: ${j.steps.find((s) => s.state === 'failed')?.error ?? 'failed'}`)
      }
    } catch (e) {
      setStatus(String(e))
    }
  }

  const rows = shown.slice(0, 30)

  return (
    <Box flexDirection="column">
      <Text>
        remodeco {session.mode} {session.status} {session.id} {changes.length} changes / {ops.length}
      </Text>
      <Text dimColor>{session.planPath}</Text>
      <Text>[e] Execute [c] Cancel [o] Re-open editor</Text>
      {confirmTrash ? <Text color="yellow">Trash ids {confirmTrash.join(', ')}? y/N</Text> : null}
      {status ? <Text color="cyan">{status}</Text> : null}
      {rows.map((o) => {
        if (o.kind === 'trash') {
          return (
            <Text key={o.id} color="red">
              delete {`{${o.id}}`} {o.from}
            </Text>
          )
        }
        if (o.kind === 'noop') {
          return (
            <Text key={o.id} dimColor>
              {`{${o.id}}`} {o.from}
            </Text>
          )
        }
        const parts = diffWords(o.from, o.to)
        return (
          <Text key={o.id}>
            {`{${o.id}}`}{' '}
            {parts.map((p, i) => (
              <Text
                key={i}
                color={p.type === 'del' ? 'red' : p.type === 'ins' ? 'green' : undefined}
                dimColor={p.type === 'equal'}
              >
                {p.text}
              </Text>
            ))}
          </Text>
        )
      })}
      {shown.length > 30 ? <Text dimColor>… {shown.length - 30} more</Text> : null}
    </Box>
  )
}

export async function runTui(opts: {
  session: SessionRecord
  config: AppConfig
  onExecute: () => Journal
}): Promise<'ok' | 'error'> {
  const inst = await render(
    <App session={opts.session} config={opts.config} onExecute={opts.onExecute} />,
  )
  await inst.waitUntilExit()
  return 'ok'
}

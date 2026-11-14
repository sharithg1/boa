// Guard rails mounted once in App: the confirmation dialogs of
// state/guards.ts, the Commit / Rollback shortcuts, and the quit guard
// (closing the window or ⌘Q with a transaction open asks first; in the
// browser preview the page's beforeunload prompt stands in).
import { useEffect } from 'react'
import { invoke, isTauri } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { TriangleAlert } from 'lucide-react'
import { backend } from '@/lib/backend'
import { connectionColor, isProduction } from '@/lib/connTag'
import { confirmCloseTx, settle, useGuards, type Pending } from '@/state/guards'
import { dbKey, useStore } from '@/state/store'
import { openTransactions, txOf, useTx } from '@/state/tx'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { ConnectionTag } from './ConnectionTag'
import { endTransaction } from './TxControls'

export function GuardRails() {
  useTxShortcuts()
  useQuitGuard()
  const pending = useGuards((s) => s.pending)
  if (!pending) return null
  if (pending.kind === 'run') return <ConfirmRun p={pending} />
  if (pending.kind === 'close-tx') return <ConfirmCloseTx p={pending} />
  return <ChooseEndTx p={pending} />
}

function answer<T>(resolve: (v: T) => void, v: T) {
  settle()
  resolve(v)
}

function ConfirmRun({ p }: { p: Extract<Pending, { kind: 'run' }> }) {
  const prod = isProduction(p.connection)
  const color = connectionColor(p.connection)
  const statements = [...new Set(p.risks.map((r) => r.statement))]
  return (
    <AlertDialog open onOpenChange={(o) => !o && answer(p.resolve, false)}>
      <AlertDialogContent className="sm:max-w-[560px]">
        <AlertDialogHeader>
          <AlertDialogTitle>
            {prod ? 'Run a destructive statement on production?' : 'Run a destructive statement?'}
          </AlertDialogTitle>
          <AlertDialogDescription className="flex items-center gap-2">
            {color && (
              <span className="size-2 shrink-0 rounded-full" style={{ background: color }} />
            )}
            <span className="font-mono text-xs">
              {p.connection.name} · {p.database}
            </span>
            <ConnectionTag conn={p.connection} />
          </AlertDialogDescription>
        </AlertDialogHeader>
        <ul className="flex flex-col gap-1.5 text-sm">
          {p.risks.map((r, i) => (
            <li key={i} className="flex items-start gap-2 text-status-err-text">
              <TriangleAlert className="mt-0.5 size-4 shrink-0" />
              <span className="min-w-0 break-words">{r.message}</span>
            </li>
          ))}
        </ul>
        <pre
          className="max-h-48 overflow-auto rounded-md border border-border bg-editor px-3 py-2 font-mono text-xs whitespace-pre-wrap text-body select-text"
          style={color ? { borderLeft: `3px solid ${color}` } : undefined}
        >
          {statements.join(';\n\n')}
        </pre>
        <AlertDialogFooter>
          <AlertDialogCancel size="md" autoFocus>
            Cancel
          </AlertDialogCancel>
          <AlertDialogAction
            size="md"
            variant="destructive"
            onClick={() => answer(p.resolve, true)}
          >
            Run
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

const CLOSE_TEXT = {
  disconnect: ['Disconnect with an open transaction?', 'Roll back and disconnect'],
  reconnect: ['Reconnect with an open transaction?', 'Roll back and reconnect'],
  quit: ['Quit with an open transaction?', 'Roll back and quit']
} as const

function ConfirmCloseTx({ p }: { p: Extract<Pending, { kind: 'close-tx' }> }) {
  const [title, action] = CLOSE_TEXT[p.action]
  return (
    <AlertDialog open onOpenChange={(o) => !o && answer(p.resolve, false)}>
      <AlertDialogContent className="sm:max-w-[460px]">
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>
            Uncommitted changes in these transactions will be rolled back:
          </AlertDialogDescription>
        </AlertDialogHeader>
        <ul className="flex flex-col gap-1 text-sm">
          {p.open.map((o) => (
            <li key={`${o.connectionId}/${o.database}`} className="flex items-center gap-2">
              <span className="truncate">{p.names[o.connectionId] ?? o.connectionId}</span>
              <span className="font-mono text-xs text-quiet">{o.database}</span>
              <span
                className={
                  o.state === 'failed' ? 'text-xs text-status-err-text' : 'text-xs text-status-wait'
                }
              >
                {o.state === 'failed' ? 'aborted' : 'open'}
              </span>
            </li>
          ))}
        </ul>
        <AlertDialogFooter>
          <AlertDialogCancel size="md" autoFocus>
            Cancel
          </AlertDialogCancel>
          <AlertDialogAction
            size="md"
            variant="destructive"
            onClick={() => answer(p.resolve, true)}
          >
            {action}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

function ChooseEndTx({ p }: { p: Extract<Pending, { kind: 'end-tx' }> }) {
  const failed = p.state === 'failed'
  return (
    <AlertDialog open onOpenChange={(o) => !o && answer(p.resolve, null)}>
      <AlertDialogContent className="sm:max-w-[440px]">
        <AlertDialogHeader>
          <AlertDialogTitle>Switch to auto-commit?</AlertDialogTitle>
          <AlertDialogDescription>
            {failed
              ? `The transaction on ${p.database} failed; it is rolled back first.`
              : `A transaction is open on ${p.database}. Commit or roll it back first.`}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel size="md">Cancel</AlertDialogCancel>
          <Button
            size="md"
            variant={failed ? 'default' : 'outline'}
            onClick={() => answer(p.resolve, 'rollback')}
          >
            Rollback
          </Button>
          {!failed && (
            <AlertDialogAction size="md" onClick={() => answer(p.resolve, 'commit')}>
              Commit
            </AlertDialogAction>
          )}
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

const isMac = () => navigator.platform.toLowerCase().includes('mac')

/** ⌘⇧↵ commits, ⌘⇧⌫ rolls back the active database's open transaction (Ctrl+Shift elsewhere). */
function useTxShortcuts() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = isMac() ? e.metaKey : e.ctrlKey
      if (!mod || !e.shiftKey || e.altKey) return
      const commit = e.key === 'Enter'
      if (!commit && e.key !== 'Backspace') return
      const s = useStore.getState()
      if (!s.active || document.querySelector('[role=dialog],[role=alertdialog]')) return
      const { connectionId, database } = s.active
      const { state } = txOf(connectionId, database)
      if (state === 'idle' || (commit && state === 'failed')) return
      e.preventDefault()
      e.stopPropagation()
      const ws = s.workspaces[dbKey(connectionId, database)]
      const tab = ws?.tabs.find((t) => t.id === ws.activeTab)
      const title = tab?.kind === 'query' ? tab.title : database
      void endTransaction(connectionId, database, tab?.id ?? `tx:${database}`, title, commit)
    }
    // Capture phase, before the console's own ⌘↵ handling.
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [])
}

const connectionNames = () =>
  Object.fromEntries(Object.values(useStore.getState().connections).map((c) => [c.id, c.name]))

/** Asks before the app goes away with transactions open, then rolls them back. */
async function confirmQuit(): Promise<boolean> {
  const open = openTransactions(useTx.getState().sessions)
  if (!open.length) return true
  if (!(await confirmCloseTx('quit', connectionNames()))) return false
  const ids = [...new Set(open.map((o) => o.connectionId))]
  await Promise.allSettled(ids.map((id) => backend.disconnect(id)))
  return true
}

function useQuitGuard() {
  useEffect(() => {
    if (!isTauri()) {
      const onUnload = (e: BeforeUnloadEvent) => {
        if (!openTransactions(useTx.getState().sessions).length) return
        e.preventDefault()
        e.returnValue = ''
      }
      window.addEventListener('beforeunload', onUnload)
      return () => window.removeEventListener('beforeunload', onUnload)
    }
    const win = getCurrentWindow()
    const unlisten = [
      // Window close button.
      win.onCloseRequested(async (e) => {
        if (!openTransactions(useTx.getState().sessions).length) return
        e.preventDefault()
        if (await confirmQuit()) await win.destroy()
      }),
      // ⌘Q / Quit menu: the Rust side holds the exit until we call `quit`.
      listen('quit-requested', async () => {
        if (await confirmQuit()) await invoke('quit')
      })
    ]
    return () => {
      for (const u of unlisten) void u.then((f) => f())
    }
  }, [])
}

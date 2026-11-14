// Transaction controls of a query console, after DataGrip's "Tx: Auto /
// Manual" switch: the session's mode, its state as the server reported it
// (state/tx.ts), and Commit / Rollback while a transaction is open (also
// after a typed BEGIN in Auto). The mode belongs to the session, so every
// console on the database shows the same. Also the status bar indicator.
import { Check, ChevronDown, CircleAlert, Undo2 } from 'lucide-react'
import { backend } from '@/lib/backend'
import { cn } from '@/lib/utils'
import { chooseEndTx } from '@/state/guards'
import { reportError, runQuery, useStore } from '@/state/store'
import { txOf, useTxSession, type TxMode, type TxState } from '@/state/tx'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger
} from '@/components/ui/dropdown-menu'
import { ConnectionTag } from './ConnectionTag'
import { IconButton } from './IconButton'

const isMac = () => typeof navigator !== 'undefined' && /mac/i.test(navigator.platform)
export const COMMIT_KEYS = () => (isMac() ? '⌘⇧↵' : 'Ctrl+Shift+Enter')
export const ROLLBACK_KEYS = () => (isMac() ? '⌘⇧⌫' : 'Ctrl+Shift+Backspace')

const TOOL_BTN =
  'flex h-6 shrink-0 cursor-pointer items-center gap-1 rounded-md px-1.5 text-xs text-quiet outline-none hover:bg-hover-strong hover:text-foreground disabled:cursor-default disabled:opacity-40 data-[state=open]:bg-hover-strong'

/** Ends the session's transaction, run (and logged) like a typed COMMIT / ROLLBACK. */
export function endTransaction(
  connectionId: string,
  database: string,
  tabId: string,
  title: string,
  commit: boolean
) {
  return runQuery(connectionId, database, tabId, title, commit ? 'COMMIT' : 'ROLLBACK')
}

/** Switches the session's mode; leaving Manual with a transaction open asks to end it first. */
export async function switchTxMode(
  connectionId: string,
  database: string,
  tabId: string,
  title: string,
  mode: TxMode
) {
  const cur = txOf(connectionId, database)
  if (cur.mode === mode) return
  if (mode === 'auto' && cur.state !== 'idle') {
    const choice = await chooseEndTx(database, cur.state)
    if (!choice) return
    await endTransaction(connectionId, database, tabId, title, choice === 'commit')
    if (txOf(connectionId, database).state !== 'idle') return
  }
  try {
    await backend.setTxMode(connectionId, database, mode === 'manual')
  } catch (e) {
    reportError('Could not change the transaction mode', e)
  }
}

export function TxControls({
  connectionId,
  database,
  tabId,
  title
}: {
  connectionId: string
  database: string
  tabId: string
  title: string
}) {
  const conn = useStore((s) => s.connections[connectionId])
  const connected = conn?.state === 'connected'
  const running = useStore((s) => !!s.running[tabId])
  const { mode, state } = useTxSession(connectionId, database)
  const end = (commit: boolean) => endTransaction(connectionId, database, tabId, title, commit)
  const pick = (m: TxMode) => switchTxMode(connectionId, database, tabId, title, m)
  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger className={TOOL_BTN} disabled={!connected}>
          Tx: {mode === 'manual' ? 'Manual' : 'Auto'}
          <ChevronDown className="size-3" />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-44">
          <DropdownMenuItem onSelect={() => pick('auto')}>
            {mode === 'auto' ? <Check /> : <span className="size-4" />}
            Auto-commit
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => pick('manual')}>
            {mode === 'manual' ? <Check /> : <span className="size-4" />}
            Manual
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      {(mode === 'manual' || state !== 'idle') && (
        <>
          <IconButton
            label={`Commit (${COMMIT_KEYS()})`}
            side="bottom"
            disabled={!connected || running || state !== 'active'}
            onClick={() => end(true)}
          >
            <Check className="size-3.5" />
          </IconButton>
          <IconButton
            label={`Rollback (${ROLLBACK_KEYS()})`}
            side="bottom"
            disabled={!connected || running || state === 'idle'}
            onClick={() => end(false)}
            className={cn(state === 'failed' && 'text-status-err-text')}
          >
            <Undo2 className="size-3.5" />
          </IconButton>
        </>
      )}
      <TxBadge state={state} />
      {conn && <ConnectionTag conn={conn} className="ml-1.5" />}
    </>
  )
}

/** "Transaction" while one is open; "Aborted" (25P02) until it is rolled back. */
export function TxBadge({ state, className }: { state: TxState; className?: string }) {
  if (state === 'idle') return null
  const failed = state === 'failed'
  return (
    <span
      role="status"
      title={
        failed
          ? 'The transaction failed: every statement is refused (SQLSTATE 25P02) until it is rolled back'
          : 'A transaction is open: commit or roll it back'
      }
      className={cn(
        'ml-1 flex h-5 shrink-0 items-center gap-1 rounded-full px-2 text-[11px]',
        failed ? 'bg-status-err/15 text-status-err-text' : 'bg-status-wait/15 text-status-wait',
        className
      )}
    >
      {failed ? (
        <CircleAlert className="size-3" />
      ) : (
        <span className="size-1.5 rounded-full bg-status-wait" />
      )}
      {failed ? 'Transaction aborted' : 'Transaction open'}
    </span>
  )
}

/** Status bar: the active session's transaction, when there is one. */
export function TxStatus() {
  const active = useStore((s) => s.active)
  const tx = useTxSession(active?.connectionId ?? '', active?.database ?? '')
  if (!active || (tx.state === 'idle' && tx.mode === 'auto')) return null
  return (
    <span className="flex shrink-0 items-center gap-1 px-1.5 text-xs text-dim">
      Tx: {tx.mode === 'manual' ? 'Manual' : 'Auto'}
      <TxBadge state={tx.state} className="ml-0" />
    </span>
  )
}

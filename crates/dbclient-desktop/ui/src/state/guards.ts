// Confirmations the guard rails ask for, as promises the run / disconnect
// flows await; components/GuardDialogs.tsx renders whichever is pending.
// - before a destructive statement (lib/guard.ts), unless turned off in
//   Settings; production-tagged connections always ask;
// - before disconnecting, reconnecting or quitting with a transaction open.

import { create } from 'zustand'
import { destructiveRisks, needsConfirm, type Risk } from '@/lib/guard'
import { isProduction } from '@/lib/connTag'
import { usePrefs } from './prefs'
import { openTransactions, useTx } from './tx'
import type { Connection } from './types'

export type OpenTx = { connectionId: string; database: string; state: 'active' | 'failed' }

export type Pending =
  | {
      kind: 'run'
      connection: Connection
      database: string
      sql: string
      risks: Risk[]
      resolve: (ok: boolean) => void
    }
  | {
      kind: 'close-tx'
      /** What closes the sessions. */
      action: 'disconnect' | 'reconnect' | 'quit'
      /** Connection names by id, for the list. */
      names: Record<string, string>
      open: OpenTx[]
      resolve: (ok: boolean) => void
    }
  | {
      kind: 'end-tx'
      /** Leaving Manual mode with a transaction open: commit or roll it back first. */
      database: string
      state: 'active' | 'failed'
      resolve: (choice: 'commit' | 'rollback' | null) => void
    }

export const useGuards = create<{ pending: Pending | null }>(() => ({ pending: null }))

/** Shows `p`; one at a time, an earlier one is answered "no". */
function ask(p: Pending) {
  const prev = useGuards.getState().pending
  if (prev) {
    if (prev.kind === 'end-tx') prev.resolve(null)
    else prev.resolve(false)
  }
  useGuards.setState({ pending: p })
}

export function settle() {
  useGuards.setState({ pending: null })
}

/** Resolves true to run `sql` (nothing risky, confirmation off, or confirmed). */
export function confirmRun(
  connection: Connection,
  database: string,
  sql: string
): Promise<boolean> {
  const risks = destructiveRisks(sql)
  const confirm = needsConfirm(risks, {
    confirmDestructive: usePrefs.getState().confirmDestructive,
    production: isProduction(connection)
  })
  if (!confirm) return Promise.resolve(true)
  return new Promise((resolve) => ask({ kind: 'run', connection, database, sql, risks, resolve }))
}

/**
 * Resolves true to go ahead with closing the sessions of `connections`
 * (all when omitted): no transaction is open, or the user accepts that it
 * is rolled back.
 */
export function confirmCloseTx(
  action: 'disconnect' | 'reconnect' | 'quit',
  names: Record<string, string>,
  connectionId?: string
): Promise<boolean> {
  const open = openTransactions(useTx.getState().sessions, connectionId) as OpenTx[]
  if (!open.length) return Promise.resolve(true)
  return new Promise((resolve) => ask({ kind: 'close-tx', action, names, open, resolve }))
}

export function chooseEndTx(
  database: string,
  state: 'active' | 'failed'
): Promise<'commit' | 'rollback' | null> {
  return new Promise((resolve) => ask({ kind: 'end-tx', database, state, resolve }))
}

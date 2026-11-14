// Transaction mode and state per session. A session is one (connection,
// database) pair on the Rust side, shared by every console and table tab on
// that database, so this is keyed like the workspaces (`dbKey`). The state
// is whatever the backend reported after the session's last statement
// (`QueryResult.tx` / `BackendError.tx`): `trackTx` wraps the backend so
// every call, from a console or a table tab, keeps it current. Connecting
// or disconnecting drops a connection's sessions (back to Auto, idle).

import { create } from 'zustand'
import type { Backend, BackendError } from '@/lib/backend'

export type TxState = 'idle' | 'active' | 'failed'
export type TxMode = 'auto' | 'manual'
export type TxSession = { mode: TxMode; state: TxState }

export const IDLE_SESSION: TxSession = { mode: 'auto', state: 'idle' }

export const useTx = create<{ sessions: Record<string, TxSession> }>(() => ({ sessions: {} }))

const key = (connectionId: string, database: string) => `${connectionId}\u0000${database}`

export function txOf(connectionId: string, database: string): TxSession {
  return useTx.getState().sessions[key(connectionId, database)] ?? IDLE_SESSION
}

/** React hook form of `txOf`. */
export function useTxSession(connectionId: string, database: string): TxSession {
  return useTx((s) => s.sessions[key(connectionId, database)] ?? IDLE_SESSION)
}

export function noteTx(connectionId: string, database: string, patch: Partial<TxSession>) {
  const k = key(connectionId, database)
  useTx.setState((s) => {
    const prev = s.sessions[k] ?? IDLE_SESSION
    const next = { ...prev, ...patch }
    if (next.mode === prev.mode && next.state === prev.state && s.sessions[k]) return {}
    return { sessions: { ...s.sessions, [k]: next } }
  })
}

/** Drops a connection's sessions (its server sessions are gone). */
export function forgetTx(connectionId: string) {
  useTx.setState((s) => ({
    sessions: Object.fromEntries(
      Object.entries(s.sessions).filter(([k]) => !k.startsWith(`${connectionId}\u0000`))
    )
  }))
}

/** Sessions with a transaction open (or aborted), optionally of one connection. */
export function openTransactions(
  sessions: Record<string, TxSession>,
  connectionId?: string
): { connectionId: string; database: string; state: TxState }[] {
  return Object.entries(sessions).flatMap(([k, s]) => {
    if (s.state === 'idle') return []
    const [cid, database] = k.split('\u0000') as [string, string]
    return connectionId && cid !== connectionId
      ? []
      : [{ connectionId: cid, database, state: s.state }]
  })
}

/** The backend, reporting every session's transaction state to `useTx`. */
export function trackTx(b: Backend): Backend {
  return {
    ...b,
    runQuery: async (id, database, sql, limit, offset) => {
      try {
        const r = await b.runQuery(id, database, sql, limit, offset)
        if (r.tx) noteTx(id, database, { state: r.tx })
        return r
      } catch (e) {
        const tx = (e as BackendError | undefined)?.tx
        if (tx) noteTx(id, database, { state: tx })
        throw e
      }
    },
    setTxMode: async (id, database, manual) => {
      const state = await b.setTxMode(id, database, manual)
      noteTx(id, database, { mode: manual ? 'manual' : 'auto', state })
      return state
    },
    connect: async (id, password) => {
      try {
        return await b.connect(id, password)
      } finally {
        forgetTx(id)
      }
    },
    disconnect: async (id) => {
      try {
        await b.disconnect(id)
      } finally {
        forgetTx(id)
      }
    },
    deleteConnection: async (id) => {
      await b.deleteConnection(id)
      forgetTx(id)
    }
  }
}

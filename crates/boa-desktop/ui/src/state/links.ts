// Notices connections dropping. `trackLinks` wraps the backend so a call on
// an open connection that fails because the server can't be reached (or
// the connection dropped under it), or because a transaction was lost
// with a dropped session, is reported to the listener the store installs
// (state/store.ts reconnects and warns). Kept apart from the store so
// lib/backend.ts can use it without an import cycle.

import type { Backend, BackendError } from '@/lib/backend'

export type LinkEvent =
  /** A call failed on the network: the connection may be down. */
  | { kind: 'lost'; connectionId: string }
  /** A session dropped with a transaction open; the server rolled it back. */
  | { kind: 'txLost'; connectionId: string; database: string }

/** Codes of calls that failed because the server couldn't be reached or the connection dropped. */
export const NETWORK_CODES: ReadonlySet<string> = new Set(['connection_lost', 'unreachable'])

let listener: ((e: LinkEvent) => void) | null = null

export function onLinkEvent(fn: ((e: LinkEvent) => void) | null) {
  listener = fn
}

function report(e: unknown, connectionId: string, database: string) {
  const code = (e as BackendError | undefined)?.code
  if (!code || !listener) return
  if (code === 'tx_lost') listener({ kind: 'txLost', connectionId, database })
  // `tx_lost` and `connection_lost` both mean a session dropped: check the rest.
  if (code === 'tx_lost' || NETWORK_CODES.has(code)) listener({ kind: 'lost', connectionId })
}

/** The backend, reporting dropped connections from every call on an open one. */
export function trackLinks(b: Backend): Backend {
  const watch =
    <A extends [string, string, ...unknown[]], R>(f: (...a: A) => Promise<R>) =>
    async (...a: A): Promise<R> => {
      try {
        return await f(...a)
      } catch (e) {
        report(e, a[0], a[1])
        throw e
      }
    }
  return {
    ...b,
    listSchemas: watch(b.listSchemas),
    introspect: watch(b.introspect),
    tableDdl: watch(b.tableDdl),
    runQuery: watch(b.runQuery),
    setTxMode: watch(b.setTxMode),
    applyEdits: watch(b.applyEdits),
    exportCsv: watch(b.exportCsv)
  }
}

// Persistent query history (DataGrip's Query History): the queries run from
// a console in the last `historyDays` (Settings; 30 by default) are kept per
// connection on disk (through lib/backend.ts; localStorage in the browser
// mock). Running a query again, even with other values, replaces its earlier
// record (lib/history.ts `queryKey`). Lists load lazily, the
// first time the history view needs a connection, and then follow new runs
// in memory. The Output log (store.ts `history`) stays a separate,
// session-only list.
import { create } from 'zustand'
import { backend } from '@/lib/backend'
import {
  dedupeQueries,
  HISTORY_CAP,
  historyCutoff,
  mergeLoaded,
  recordKey,
  toRecord,
  type HistoryRecord
} from '@/lib/history'
import { onPrefsChange, usePrefs } from './prefs'
import type { HistoryEntry } from './types'

type HistoryState = {
  /** The Query History view is open. */
  open: boolean
  /** Loaded histories by connection id, newest first. */
  lists: Record<string, HistoryRecord[]>
  /** Connections whose history is being read. */
  loading: Record<string, true>
}

export const useQueryHistory = create<HistoryState>(() => ({
  open: false,
  lists: {},
  loading: {}
}))

export function openHistory() {
  useQueryHistory.setState({ open: true })
}

export function toggleHistory() {
  useQueryHistory.setState((s) => ({ open: !s.open }))
}

/** Records made while a connection's history was loading, merged in afterwards. */
const pending = new Map<string, HistoryRecord[]>()

function reportError(title: string, e: unknown) {
  // Lazy: store.ts imports this module.
  void import('./store').then((m) => m.reportError(title, e))
}

let appendFailed = false

/**
 * Saves a statement the user ran (not page re-fetches) to its connection's
 * history, unless recording is turned off in Settings.
 */
export function recordHistory(connectionId: string, entry: HistoryEntry) {
  const { recordHistory: on, historyDays: days } = usePrefs.getState()
  if (!on) return
  const rec = toRecord(entry)
  const s = useQueryHistory.getState()
  // Earlier runs of this query saved without a key (before runs were
  // deduplicated): the backend cannot tell they are the same query, so
  // remove them here, or they would come back once this run is deleted.
  const legacy = (s.lists[connectionId] ?? [])
    .filter((r) => r.key === undefined && recordKey(r) === rec.key)
    .map((r) => r.id)
  if (legacy.length) backend.historyDelete(connectionId, legacy).catch(() => {})
  if (s.lists[connectionId]) {
    useQueryHistory.setState((st) => ({
      lists: {
        ...st.lists,
        [connectionId]: dedupeQueries(
          [rec, ...(st.lists[connectionId] ?? [])],
          HISTORY_CAP,
          historyCutoff(days)
        )
      }
    }))
  }
  // A (re)load in flight may read the file before this append lands.
  if (s.loading[connectionId])
    pending.set(connectionId, [rec, ...(pending.get(connectionId) ?? [])])
  backend.historyAppend(connectionId, rec, days).then(
    () => {
      appendFailed = false
    },
    (e) => {
      // One toast per failure streak, not one per statement.
      if (!appendFailed) reportError('Could not save query history', e)
      appendFailed = true
    }
  )
}

/** Reads a connection's history (once; `force` re-reads). */
export async function loadHistory(connectionId: string, { force = false } = {}) {
  const s = useQueryHistory.getState()
  if (s.loading[connectionId] || (s.lists[connectionId] && !force)) return
  useQueryHistory.setState((st) => ({ loading: { ...st.loading, [connectionId]: true } }))
  pending.set(connectionId, [])
  try {
    const days = usePrefs.getState().historyDays
    const list = await backend.historyList(connectionId, days)
    const merged = mergeLoaded(
      [...list].reverse(),
      pending.get(connectionId) ?? [],
      HISTORY_CAP,
      historyCutoff(days)
    )
    useQueryHistory.setState((st) => ({ lists: { ...st.lists, [connectionId]: merged } }))
    // Earlier runs of a query that is listed (saved before runs were
    // deduplicated): remove them, or they would resurface when the listed
    // one is deleted.
    const kept = new Set(merged.map((r) => r.id))
    const stale = list.filter((r) => !kept.has(r.id)).map((r) => r.id)
    if (stale.length) backend.historyDelete(connectionId, stale).catch(() => {})
  } catch (e) {
    reportError('Could not read query history', e)
  } finally {
    pending.delete(connectionId)
    useQueryHistory.setState((st) => {
      const { [connectionId]: _, ...loading } = st.loading
      return { loading }
    })
  }
}

export async function deleteHistoryEntries(connectionId: string, ids: string[]) {
  const drop = new Set(ids)
  useQueryHistory.setState((st) => {
    const list = st.lists[connectionId]
    return list
      ? { lists: { ...st.lists, [connectionId]: list.filter((r) => !drop.has(r.id)) } }
      : {}
  })
  try {
    await backend.historyDelete(connectionId, ids)
  } catch (e) {
    reportError('Could not delete history entry', e)
    void loadHistory(connectionId, { force: true })
  }
}

export async function clearHistory(connectionId: string) {
  useQueryHistory.setState((st) => ({ lists: { ...st.lists, [connectionId]: [] } }))
  try {
    await backend.historyClear(connectionId)
  } catch (e) {
    reportError('Could not clear query history', e)
    void loadHistory(connectionId, { force: true })
  }
}

// A new retention period: re-read what is loaded (the files keep runs past
// a shorter period until the next statement is saved).
onPrefsChange((next, prev) => {
  if (next.historyDays === prev.historyDays) return
  for (const id of Object.keys(useQueryHistory.getState().lists))
    void loadHistory(id, { force: true })
})

/** Drops a deleted connection's list (the backend deletes its file). */
export function forgetHistory(connectionId: string) {
  pending.delete(connectionId)
  useQueryHistory.setState((st) => {
    const { [connectionId]: _, ...lists } = st.lists
    return { lists }
  })
}

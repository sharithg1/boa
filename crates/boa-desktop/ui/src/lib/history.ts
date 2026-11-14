// Persistent query history: the record shape stored per connection (the
// Rust side's `history.rs` keeps it as JSON lines), search, previews, and
// the localStorage store the browser mock uses in place of the files.
import type { HistoryEntry } from '@/state/types'

/** One executed statement as stored on disk. */
export type HistoryRecord = {
  id: string
  sql: string
  database: string
  /** When it ran (ms since the epoch). */
  at: number
  /** Duration in ms. */
  ms: number
  /** Rows fetched, for row-returning statements. */
  rows?: number
  /** More rows existed than were fetched. */
  truncated?: boolean
  /** Affected rows, for statements that return none. */
  affected?: number
  /** Editor tab it ran from (`console_1`). */
  source?: string
  error?: string
  /** Server detail / hint lines of an error. */
  detail?: string
}

/** A record with the connection it belongs to, as the history view lists it. */
export type HistoryItem = HistoryRecord & { connectionId: string }

/** Entries kept per connection; matches `CAP` in `src/history.rs`. */
export const HISTORY_CAP = 5000

export function newHistoryId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function')
    return crypto.randomUUID()
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`
}

/** The stored form of an Output log entry (no session-only fields, no nulls). */
export function toRecord(e: HistoryEntry, id = newHistoryId()): HistoryRecord {
  const r: HistoryRecord = { id, sql: e.sql, database: e.database, at: e.at, ms: e.ms }
  if (e.rows != null) r.rows = e.rows
  if (e.truncated) r.truncated = true
  if (e.affected != null) r.affected = e.affected
  if (e.source) r.source = e.source
  if (e.error) r.error = e.error
  if (e.detail) r.detail = e.detail
  return r
}

// ── Search ────────────────────────────────────────────────────────────────

/** Lower-cased SQL per record, computed once (records are never mutated). */
const lowered = new WeakMap<HistoryRecord, string>()
function lower(r: HistoryRecord): string {
  let s = lowered.get(r)
  if (s === undefined) {
    s = r.sql.toLowerCase()
    lowered.set(r, s)
  }
  return s
}

export function searchTerms(query: string): string[] {
  return query.toLowerCase().split(/\s+/).filter(Boolean)
}

export type HistoryFilter = { query: string; errorsOnly?: boolean }

/**
 * Records whose SQL contains every whitespace-separated term of `query`
 * (case-insensitive, any order); `errorsOnly` keeps failed ones. Order is kept.
 */
export function filterHistory<T extends HistoryRecord>(items: readonly T[], f: HistoryFilter): T[] {
  const terms = searchTerms(f.query)
  if (!terms.length && !f.errorsOnly) return items as T[]
  return items.filter((r) => {
    if (f.errorsOnly && !r.error) return false
    if (!terms.length) return true
    const s = lower(r)
    for (const t of terms) if (!s.includes(t)) return false
    return true
  })
}

/** Lists of several connections (each newest first), merged newest first. */
export function mergeNewestFirst<T extends HistoryRecord>(lists: readonly (readonly T[])[]): T[] {
  return lists.flat().sort((a, b) => b.at - a.at)
}

/**
 * Prepends records made while a load was in flight to the loaded list
 * (newest first), skipping ones the load already returned, and caps it.
 */
export function mergeLoaded(
  loaded: readonly HistoryRecord[],
  pending: readonly HistoryRecord[],
  cap = HISTORY_CAP
): HistoryRecord[] {
  const seen = new Set(loaded.map((r) => r.id))
  const extra = pending.filter((r) => !seen.has(r.id))
  return mergeNewestFirst([extra, loaded]).slice(0, cap)
}

// ── Display ───────────────────────────────────────────────────────────────

/** The first `lines` non-blank lines of a statement, and whether there is more. */
export function sqlPreview(sql: string, lines = 2): { text: string; more: boolean } {
  const all = sql.replace(/\r\n?/g, '\n').split('\n')
  const start = all.findIndex((l) => l.trim() !== '')
  if (start < 0) return { text: '', more: false }
  let end = all.length
  while (end > start && all[end - 1]!.trim() === '') end--
  const kept = all.slice(start, Math.min(end, start + lines))
  return { text: kept.join('\n'), more: end - start > lines }
}

/** "12 rows", "12+ rows", "3 affected", "error", "done". */
export function outcomeLabel(r: HistoryRecord): string {
  if (r.error) return 'error'
  if (r.rows != null)
    return `${r.rows.toLocaleString()}${r.truncated ? '+' : ''} row${r.rows === 1 && !r.truncated ? '' : 's'}`
  if (r.affected) return `${r.affected.toLocaleString()} affected`
  return 'done'
}

/** "850 ms", "1.2 s", "2m 5s". */
export function duration(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)} ms`
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 1 : 0)} s`
  const s = Math.round(ms / 1000)
  return `${Math.floor(s / 60)}m ${s % 60}s`
}

/**
 * Text to insert at the caret of a console: the statement on a line of its
 * own when the caret is at the end (or start) of a line holding other text;
 * as is in the middle of a line (e.g. as a subquery).
 */
export function insertionText(before: string, after: string, sql: string): string {
  const lineBefore = before.slice(before.lastIndexOf('\n') + 1)
  const nl = after.indexOf('\n')
  const lineAfter = nl < 0 ? after : after.slice(0, nl)
  const textBefore = lineBefore.trim() !== ''
  const textAfter = lineAfter.trim() !== ''
  if (textBefore && !textAfter) return `\n${sql}`
  if (!textBefore && textAfter) return `${sql}\n`
  return sql
}

// ── Browser mock storage ──────────────────────────────────────────────────

type KV = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>

/** The history files' operations over localStorage (oldest first, capped). */
export function localHistoryStore(kv: KV | undefined, cap = HISTORY_CAP) {
  const key = (id: string) => `dbclient.history.${id}`
  const read = (id: string): HistoryRecord[] => {
    try {
      const raw = kv?.getItem(key(id))
      const list = raw ? (JSON.parse(raw) as unknown) : []
      return Array.isArray(list) ? (list as HistoryRecord[]) : []
    } catch {
      return []
    }
  }
  const write = (id: string, list: HistoryRecord[]) => {
    try {
      kv?.setItem(key(id), JSON.stringify(list))
    } catch {
      // Quota or storage unavailable: history is best effort in the mock.
    }
  }
  return {
    list: (id: string) => read(id),
    append: (id: string, r: HistoryRecord) => {
      const list = read(id)
      list.push(r)
      write(id, list.slice(-cap))
    },
    remove: (id: string, ids: string[]) => {
      const drop = new Set(ids)
      write(
        id,
        read(id).filter((r) => !drop.has(r.id))
      )
    },
    clear: (id: string) => {
      try {
        kv?.removeItem(key(id))
      } catch {
        // ignore
      }
    }
  }
}

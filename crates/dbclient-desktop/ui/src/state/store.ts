// App state (zustand): connections and their schemas, the selected
// database, its tabs (queries and open tables), panel toggles, overlays and
// toasts. Connection actions are simulated against state/mock.ts until the
// Rust side has drivers.

import { create } from 'zustand'
import { sampleConnections } from './mock'
import { execute } from './mockData'
import { usePrefs } from './prefs'
import type { Connection, HistoryEntry, ResultSet, Table } from './types'

export type QueryTab = { id: string; kind: 'query'; title: string; sql: string }
export type TableTab = {
  id: string
  kind: 'table'
  schema: string
  table: string
  /** Italic preview tab, replaced by the next single-click open (double-click pins). */
  preview?: boolean
}
export type Tab = QueryTab | TableTab

export type Workspace = { tabs: Tab[]; activeTab: string | null }

export type Active = { connectionId: string; database: string }

export type SettingsPage = 'general' | 'connections' | 'keyboard'

export type Toast = {
  id: number
  kind: 'info' | 'success' | 'error'
  title: string
  body?: string
  action?: { label: string; run: () => void }
}

type State = {
  connections: Record<string, Connection>
  connectionOrder: string[]
  active: Active | null
  workspaces: Record<string, Workspace>
  /** Recently opened tables, newest first, as `tableKey`s. */
  mru: string[]
  /** When each table was last opened (ms), by `tableKey`. */
  seen: Record<string, number>
  history: Record<string, HistoryEntry[]>
  sidebarOpen: boolean
  rightPanelOpen: boolean
  /** Bottom "Database Output" tool window. */
  bottomOpen: boolean
  /** What it shows: the Output log, or the result of an editor tab (by tab id). */
  bottomView: 'output' | string
  /** Latest result of each editor tab, by tab id. */
  results: Record<string, ResultSet>
  /** Tabs with a statement in flight, by tab id. */
  running: Record<string, true>
  /** Caret of the focused SQL editor (1-based), for the status bar. */
  cursor: { line: number; col: number } | null
  paletteOpen: boolean
  settings: SettingsPage | null
  toasts: Toast[]
}

const initial = sampleConnections()

export const useStore = create<State>(() => ({
  connections: Object.fromEntries(initial.map((c) => [c.id, c])),
  connectionOrder: initial.map((c) => c.id),
  active: null,
  workspaces: {},
  mru: [],
  seen: {},
  history: {},
  sidebarOpen: true,
  rightPanelOpen: typeof window === 'undefined' || window.innerWidth >= 1100,
  bottomOpen: true,
  bottomView: 'output',
  results: {},
  running: {},
  cursor: null,
  paletteOpen: false,
  settings: null,
  toasts: []
}))

// ── Keys ──────────────────────────────────────────────────────────────────

export const dbKey = (connectionId: string, database: string) => `${connectionId}\u0000${database}`

export const tableKey = (connectionId: string, database: string, schema: string, table: string) =>
  `${connectionId}\u0000${database}\u0000${schema}\u0000${table}`

export function parseTableKey(key: string) {
  const [connectionId, database, schema, table] = key.split('\u0000') as [
    string,
    string,
    string,
    string
  ]
  return { connectionId, database, schema, table }
}

let nextId = 1
const newId = (p: string) => `${p}${nextId++}`

// ── Lookups ───────────────────────────────────────────────────────────────

export function findTable(
  connectionId: string,
  database: string,
  schema: string,
  table: string
): Table | undefined {
  return useStore
    .getState()
    .connections[connectionId]?.databases.find((d) => d.name === database)
    ?.schemas.find((s) => s.name === schema)
    ?.tables.find((t) => t.name === table)
}

/** Every database of every connection, in sidebar order. */
export function allDatabases(): Active[] {
  const s = useStore.getState()
  return s.connectionOrder.flatMap((id) =>
    (s.connections[id]?.databases ?? []).map((d) => ({ connectionId: id, database: d.name }))
  )
}

// ── Selection and tabs ────────────────────────────────────────────────────

function patchWorkspace(key: string, f: (ws: Workspace) => Workspace) {
  useStore.setState((s) => ({
    workspaces: { ...s.workspaces, [key]: f(s.workspaces[key] ?? { tabs: [], activeTab: null }) }
  }))
}

export function selectDatabase(connectionId: string, database: string) {
  useStore.setState({ active: { connectionId, database } })
}

/** Opens a table in a tab (a preview tab unless `pin`), selecting its database. */
export function openTable(
  connectionId: string,
  database: string,
  schema: string,
  table: string,
  { pin = false }: { pin?: boolean } = {}
) {
  const key = dbKey(connectionId, database)
  const tk = tableKey(connectionId, database, schema, table)
  selectDatabase(connectionId, database)
  useStore.setState((s) => ({
    mru: [tk, ...s.mru.filter((k) => k !== tk)].slice(0, 30),
    seen: { ...s.seen, [tk]: Date.now() }
  }))
  patchWorkspace(key, (ws) => {
    const existing = ws.tabs.find(
      (t) => t.kind === 'table' && t.schema === schema && t.table === table
    )
    if (existing) {
      const tabs = pin
        ? ws.tabs.map((t) => (t.id === existing.id ? { ...t, preview: false } : t))
        : ws.tabs
      return { tabs, activeTab: existing.id }
    }
    const tab: TableTab = { id: newId('t'), kind: 'table', schema, table, preview: !pin }
    const preview = ws.tabs.findIndex((t) => t.kind === 'table' && t.preview)
    const tabs = [...ws.tabs]
    if (preview >= 0 && !pin) tabs[preview] = tab
    else tabs.push(tab)
    return { tabs, activeTab: tab.id }
  })
}

export function newQueryTab(connectionId: string, database: string, sql = '') {
  const key = dbKey(connectionId, database)
  selectDatabase(connectionId, database)
  patchWorkspace(key, (ws) => {
    // DataGrip-style console names: the lowest free console_N.
    const used = new Set(ws.tabs.map((t) => (t.kind === 'query' ? t.title : '')))
    let n = 1
    while (used.has(`console_${n}`)) n++
    const tab: QueryTab = { id: newId('q'), kind: 'query', title: `console_${n}`, sql }
    return { tabs: [...ws.tabs, tab], activeTab: tab.id }
  })
}

export function setQuerySql(key: string, id: string, sql: string) {
  patchWorkspace(key, (ws) => ({
    ...ws,
    tabs: ws.tabs.map((t) => (t.id === id && t.kind === 'query' ? { ...t, sql } : t))
  }))
}

export function activateTab(key: string, id: string) {
  patchWorkspace(key, (ws) => ({ ...ws, activeTab: id }))
}

export function activateTabIndex(i: number) {
  const { active, workspaces } = useStore.getState()
  if (!active) return
  const key = dbKey(active.connectionId, active.database)
  const tab = workspaces[key]?.tabs[i]
  if (tab) activateTab(key, tab.id)
}

export function pinTab(key: string, id: string) {
  patchWorkspace(key, (ws) => ({
    ...ws,
    tabs: ws.tabs.map((t) => (t.id === id && t.kind === 'table' ? { ...t, preview: false } : t))
  }))
}

export function closeTab(key: string, id: string) {
  cancelQuery(id)
  useStore.setState((s) => {
    if (!s.results[id]) return {}
    const { [id]: _, ...results } = s.results
    return { results, bottomView: s.bottomView === id ? 'output' : s.bottomView }
  })
  patchWorkspace(key, (ws) => {
    const i = ws.tabs.findIndex((t) => t.id === id)
    if (i < 0) return ws
    const tabs = ws.tabs.filter((t) => t.id !== id)
    const activeTab =
      ws.activeTab === id ? (tabs[Math.min(i, tabs.length - 1)]?.id ?? null) : ws.activeTab
    return { tabs, activeTab }
  })
}

export function moveTab(key: string, id: string, to: number) {
  patchWorkspace(key, (ws) => {
    const from = ws.tabs.findIndex((t) => t.id === id)
    if (from < 0 || to < 0 || to >= ws.tabs.length) return ws
    const tabs = [...ws.tabs]
    const [tab] = tabs.splice(from, 1)
    tabs.splice(to, 0, tab!)
    return { ...ws, tabs }
  })
}

export function tabLabel(t: Tab): string {
  return t.kind === 'query' ? t.title : t.table
}

/** Next / previous database in sidebar order (⌥↑ / ⌥↓). */
export function cycleDatabase(d: 1 | -1) {
  const list = allDatabases()
  if (!list.length) return
  const { active } = useStore.getState()
  const i = active
    ? list.findIndex(
        (x) => x.connectionId === active.connectionId && x.database === active.database
      )
    : -1
  const next = list[(i + d + list.length) % list.length]!
  selectDatabase(next.connectionId, next.database)
}

// ── Connections (simulated) ───────────────────────────────────────────────

function patchConnection(id: string, patch: Partial<Connection>) {
  useStore.setState((s) => {
    const c = s.connections[id]
    return c ? { connections: { ...s.connections, [id]: { ...c, ...patch } } } : {}
  })
}

export function connect(id: string) {
  patchConnection(id, { state: 'connecting', latencyMs: null })
  setTimeout(() => {
    const c = useStore.getState().connections[id]
    if (c?.state !== 'connecting') return
    patchConnection(id, {
      state: 'connected',
      latencyMs: c.engine === 'sqlite' ? 0 : 12 + Math.round(Math.random() * 30)
    })
  }, 700)
}

export function disconnect(id: string) {
  patchConnection(id, { state: 'disconnected', latencyMs: null })
}

/** Mock only: drop the link and come back, to see the states. */
export function simulateDrop(id: string) {
  patchConnection(id, { state: 'failed', latencyMs: null })
  pushToast({ kind: 'error', title: 'Connection lost', body: 'Reconnecting…' }, 2400)
  setTimeout(() => connect(id), 1500)
}

export function connectionString(c: Connection, database?: string): string {
  if (c.engine === 'sqlite') return `sqlite://${c.host}`
  const auth = c.user ? `${c.user}@` : ''
  const port = c.port ? `:${c.port}` : ''
  return `${c.engine}://${auth}${c.host}${port}${database ? `/${database}` : ''}`
}

// ── Queries (simulated) ───────────────────────────────────────────────────

const timers: Record<string, ReturnType<typeof setTimeout>> = {}

function log(connectionId: string, entry: HistoryEntry) {
  useStore.setState((s) => ({
    history: {
      ...s.history,
      [connectionId]: [entry, ...(s.history[connectionId] ?? [])].slice(0, 200)
    }
  }))
}

/**
 * Runs one statement for an editor tab (simulated against state/mockData.ts):
 * rows land in the tab's result and the bottom panel switches to it; other
 * statements and errors are logged to the Output view.
 */
export function runQuery(
  connectionId: string,
  database: string,
  tabId: string,
  title: string,
  sql: string
) {
  const text = sql.trim()
  if (!text || useStore.getState().running[tabId]) return
  const conn = useStore.getState().connections[connectionId]
  const at = Date.now()
  if (conn?.state !== 'connected') {
    log(connectionId, {
      id: nextId++,
      sql: text,
      database,
      at,
      ms: 0,
      rows: null,
      source: title,
      error: `${conn?.name ?? 'Connection'} is not connected`
    })
    useStore.setState({ bottomOpen: true, bottomView: 'output' })
    return
  }
  const ms = 4 + Math.round(Math.random() * (/^\s*select/i.test(text) ? 600 : 80))
  useStore.setState((s) => ({ running: { ...s.running, [tabId]: true } }))
  timers[tabId] = setTimeout(() => {
    delete timers[tabId]
    useStore.setState((s) => {
      const { [tabId]: _, ...running } = s.running
      return { running }
    })
    const db = useStore
      .getState()
      .connections[connectionId]?.databases.find((d) => d.name === database)
    const res = execute(db, text, usePrefs.getState().rowLimit)
    const base = { id: nextId++, sql: text, database, at, ms, source: title }
    if (res.kind === 'error') {
      log(connectionId, { ...base, rows: null, error: res.message })
      useStore.setState({ bottomOpen: true, bottomView: 'output' })
    } else if (res.kind === 'affected') {
      log(connectionId, { ...base, rows: null, affected: res.count })
      useStore.setState({ bottomOpen: true, bottomView: 'output' })
    } else {
      log(connectionId, { ...base, rows: res.rows.length })
      const result: ResultSet = {
        tabId,
        title: res.table ?? title,
        sql: text,
        columns: res.columns,
        rows: res.rows,
        ms,
        at
      }
      useStore.setState((s) => ({
        results: { ...s.results, [tabId]: result },
        bottomOpen: true,
        bottomView: tabId
      }))
    }
  }, ms)
}

export function cancelQuery(tabId: string) {
  const t = timers[tabId]
  if (!t) return
  clearTimeout(t)
  delete timers[tabId]
  useStore.setState((s) => {
    const { [tabId]: _, ...running } = s.running
    return { running }
  })
}

export function closeResult(tabId: string) {
  useStore.setState((s) => {
    const { [tabId]: _, ...results } = s.results
    return { results, bottomView: s.bottomView === tabId ? 'output' : s.bottomView }
  })
}

// ── Toasts ────────────────────────────────────────────────────────────────

export function pushToast(t: Omit<Toast, 'id'>, ttlMs = 6000): number {
  const id = nextId++
  useStore.setState((s) => ({ toasts: [...s.toasts, { ...t, id }].slice(-5) }))
  if (ttlMs > 0) setTimeout(() => dismissToast(id), ttlMs)
  return id
}

export function dismissToast(id: number) {
  useStore.setState((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) }))
}

export function reportError(title: string, err: unknown) {
  pushToast({ kind: 'error', title, body: err instanceof Error ? err.message : String(err) })
}

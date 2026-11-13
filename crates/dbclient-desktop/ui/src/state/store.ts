// App state (zustand): connections and their schemas, the selected
// database, its tabs (queries and open tables), panel toggles, overlays and
// toasts. Connections, introspection and queries go through lib/backend.ts
// (the Rust side in the app, a mock in a plain browser).

import { create } from 'zustand'
import { backend, toBackendError, type ConnConfig, type SavedConnection } from '@/lib/backend'
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
  /** New / edit connection dialog: `{}` for a new one, `{ id }` to edit. */
  connectionDialog: { id?: string } | null
  /** Asks for the password of a connection that has none stored. */
  passwordPrompt: { connectionId: string; error?: string } | null
  toasts: Toast[]
}

export const useStore = create<State>(() => ({
  connections: {},
  connectionOrder: [],
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
  connectionDialog: null,
  passwordPrompt: null,
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
  void loadDatabase(connectionId, database)
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

// ── Connections ───────────────────────────────────────────────────────────

function patchConnection(id: string, patch: Partial<Connection>) {
  useStore.setState((s) => {
    const c = s.connections[id]
    return c ? { connections: { ...s.connections, [id]: { ...c, ...patch } } } : {}
  })
}

function fromSaved(c: SavedConnection, prev?: Connection): Connection {
  return {
    id: c.id,
    name: c.name,
    engine: 'postgres',
    host: c.host,
    port: c.port,
    user: c.user,
    database: c.database,
    sslMode: c.sslMode,
    hasPassword: c.hasPassword,
    state: prev?.state ?? 'disconnected',
    latencyMs: prev?.latencyMs ?? null,
    serverVersion: prev?.serverVersion ?? null,
    databases: prev?.databases ?? []
  }
}

export function configOf(c: Connection): ConnConfig {
  return {
    id: c.id,
    name: c.name,
    host: c.host,
    port: c.port ?? 5432,
    user: c.user ?? '',
    database: c.database,
    sslMode: c.sslMode
  }
}

/** Loads the saved connections (once, at startup). */
export async function loadConnections() {
  try {
    const list = await backend.listConnections()
    useStore.setState((s) => ({
      connections: Object.fromEntries(list.map((c) => [c.id, fromSaved(c, s.connections[c.id])])),
      connectionOrder: list.map((c) => c.id)
    }))
  } catch (e) {
    reportError('Could not load connections', e)
  }
}

/** Saves a new or edited connection; a changed connection reconnects if it was open. */
export async function saveConnection(
  config: ConnConfig,
  password: string | null,
  savePassword: boolean
) {
  await backend.saveConnection(config, password, savePassword)
  const prev = useStore.getState().connections[config.id]
  const hasPassword = savePassword && (password != null || !!prev?.hasPassword)
  useStore.setState((s) => ({
    connections: {
      ...s.connections,
      [config.id]: fromSaved({ ...config, hasPassword }, prev)
    },
    connectionOrder: s.connectionOrder.includes(config.id)
      ? s.connectionOrder
      : [...s.connectionOrder, config.id]
  }))
  if (prev?.state === 'connected')
    await connect(config.id, savePassword ? undefined : (password ?? undefined))
}

export async function deleteConnection(id: string) {
  await backend.deleteConnection(id)
  useStore.setState((s) => {
    const { [id]: _, ...connections } = s.connections
    const workspaces = Object.fromEntries(
      Object.entries(s.workspaces).filter(([k]) => !k.startsWith(`${id}\u0000`))
    )
    return {
      connections,
      workspaces,
      connectionOrder: s.connectionOrder.filter((x) => x !== id),
      active: s.active?.connectionId === id ? null : s.active
    }
  })
}

/**
 * Connects (with `password`, else the stored one), lists the databases and
 * loads the default one. A server asking for a password that isn't stored
 * opens the password prompt.
 */
export async function connect(id: string, password?: string): Promise<boolean> {
  const conn = useStore.getState().connections[id]
  if (!conn || conn.state === 'connecting') return false
  patchConnection(id, { state: 'connecting', latencyMs: null })
  const started = performance.now()
  try {
    const info = await backend.connect(id, password)
    const prev = new Map(conn.databases.map((d) => [d.name, d]))
    patchConnection(id, {
      state: 'connected',
      latencyMs: Math.round(performance.now() - started),
      serverVersion: info.serverVersion,
      // Keep what was loaded before a reconnect; it refreshes on demand.
      databases: info.databases.map((name) => ({
        name,
        schemas: prev.get(name)?.schemas ?? [],
        loaded: false
      }))
    })
    useStore.setState({ passwordPrompt: null })
    // The default database, plus any left expanded in the explorer.
    const first = info.databases.includes(conn.database) ? conn.database : info.databases[0]
    const expanded = usePrefs.getState().treeExpanded
    for (const name of info.databases)
      if (name === first || expanded[`${id}/${name}`]) void loadDatabase(id, name)
    return true
  } catch (e) {
    const err = toBackendError(e)
    const auth = err.code === 'password_required' || err.code === '28P01'
    patchConnection(id, { state: auth ? 'disconnected' : 'failed' })
    if (auth && (password != null || !conn.hasPassword || err.code === '28P01')) {
      useStore.setState({
        passwordPrompt: {
          connectionId: id,
          error: password != null || err.code === '28P01' ? err.message : undefined
        }
      })
    } else {
      pushToast({ kind: 'error', title: `Could not connect to ${conn.name}`, body: err.message })
    }
    return false
  }
}

export async function disconnect(id: string) {
  patchConnection(id, { state: 'disconnected', latencyMs: null })
  try {
    await backend.disconnect(id)
  } catch (e) {
    reportError('Disconnect failed', e)
  }
}

function patchDatabase(
  id: string,
  database: string,
  patch: Partial<Connection['databases'][number]>
) {
  useStore.setState((s) => {
    const c = s.connections[id]
    if (!c) return {}
    const databases = c.databases.map((d) => (d.name === database ? { ...d, ...patch } : d))
    return { connections: { ...s.connections, [id]: { ...c, databases } } }
  })
}

/** Introspects a database's schemas, tables and columns (once, unless `force`). */
export async function loadDatabase(id: string, database: string, { force = false } = {}) {
  const c = useStore.getState().connections[id]
  const db = c?.databases.find((d) => d.name === database)
  if (c?.state !== 'connected' || !db || db.loading || (db.loaded && !force)) return
  patchDatabase(id, database, { loading: true })
  try {
    const schemas = await backend.introspect(id, database)
    patchDatabase(id, database, { schemas, loaded: true, loading: false })
  } catch (e) {
    patchDatabase(id, database, { loading: false })
    reportError(`Could not read ${database}`, e)
  }
}

export function connectionString(c: Connection, database?: string): string {
  const auth = c.user ? `${encodeURIComponent(c.user)}@` : ''
  const port = c.port ? `:${c.port}` : ''
  return `postgresql://${auth}${c.host}${port}/${database ?? c.database}`
}

// ── Queries ───────────────────────────────────────────────────────────────

function log(connectionId: string, entry: HistoryEntry) {
  useStore.setState((s) => ({
    history: {
      ...s.history,
      [connectionId]: [entry, ...(s.history[connectionId] ?? [])].slice(0, 200)
    }
  }))
}

function errorText(e: unknown): { error: string; detail?: string } {
  const err = toBackendError(e)
  const detail = [err.detail && `Detail: ${err.detail}`, err.hint && `Hint: ${err.hint}`]
    .filter(Boolean)
    .join('\n')
  return {
    error:
      err.code && /^[0-9A-Z]{5}$/.test(err.code) ? `[${err.code}] ${err.message}` : err.message,
    detail: detail || undefined
  }
}

/**
 * Runs one statement for an editor tab: rows land in the tab's result and
 * the bottom panel switches to it; other statements and errors are logged
 * to the Output view.
 */
export async function runQuery(
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
  const base = { sql: text, database, at, source: title }
  if (conn?.state !== 'connected') {
    log(connectionId, {
      ...base,
      id: nextId++,
      ms: 0,
      rows: null,
      error: `${conn?.name ?? 'Connection'} is not connected`
    })
    useStore.setState({ bottomOpen: true, bottomView: 'output' })
    return
  }
  useStore.setState((s) => ({ running: { ...s.running, [tabId]: true } }))
  try {
    const res = await backend.runQuery(connectionId, database, text, usePrefs.getState().rowLimit)
    if (res.columns.length) {
      log(connectionId, {
        ...base,
        id: nextId++,
        ms: res.ms,
        rows: res.rows.length,
        truncated: res.truncated
      })
      const result: ResultSet = {
        tabId,
        title,
        sql: text,
        columns: res.columns,
        rows: res.rows,
        truncated: res.truncated,
        ms: res.ms,
        at
      }
      useStore.setState((s) => ({
        results: { ...s.results, [tabId]: result },
        bottomOpen: true,
        bottomView: tabId
      }))
    } else {
      log(connectionId, {
        ...base,
        id: nextId++,
        ms: res.ms,
        rows: null,
        affected: res.affected ?? 0
      })
      useStore.setState({ bottomOpen: true, bottomView: 'output' })
    }
    // DDL may have changed the tree.
    if (/^\s*(create|drop|alter)\b/i.test(text))
      void loadDatabase(connectionId, database, { force: true })
  } catch (e) {
    log(connectionId, { ...base, id: nextId++, ms: Date.now() - at, rows: null, ...errorText(e) })
    useStore.setState({ bottomOpen: true, bottomView: 'output' })
    if (toBackendError(e).code === 'not_connected')
      patchConnection(connectionId, { state: 'disconnected' })
  } finally {
    useStore.setState((s) => {
      const { [tabId]: _, ...running } = s.running
      return { running }
    })
  }
}

/** Asks the server to cancel the tab's running statement (it then fails with 57014). */
export function cancelQuery(tabId: string) {
  const { running, active } = useStore.getState()
  if (!running[tabId] || !active) return
  void backend
    .cancelQuery(active.connectionId, active.database)
    .catch((e) => reportError('Cancel failed', e))
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
  pushToast({ kind: 'error', title, body: toBackendError(err).message })
}

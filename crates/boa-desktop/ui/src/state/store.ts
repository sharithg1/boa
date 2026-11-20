// App state (zustand): connections and their schemas, the selected
// database, its tabs (queries and open tables), panel toggles, overlays and
// toasts. Connections, introspection and queries go through lib/backend.ts
// (the Rust side in the app, a mock in a plain browser).

import { create } from 'zustand'
import { backend, toBackendError, type ConnConfig, type SavedConnection } from '@/lib/backend'
import { forgetHistory, recordHistory } from './history'
import { setPrefs, usePrefs } from './prefs'
import { confirmCloseTx, confirmRun } from './guards'
import { readOnlyEscape } from '@/lib/guard'
import { onLinkEvent } from './links'
import { noteTx } from './tx'
import type { BackendError } from '@/lib/backend'
import type { Connection, HistoryEntry, ResultSet, Table } from './types'

export type QueryTab = { id: string; kind: 'query'; title: string; sql: string }
export type TableTab = {
  id: string
  kind: 'table'
  schema: string
  table: string
  /** Italic preview tab, replaced by the next single-click open (double-click pins). */
  preview?: boolean
  /** Filter bar clauses (lib/tableQuery.ts), applied server-side; absent when empty. */
  where?: string
  orderBy?: string
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
  /**
   * Connections the user opened and hasn't disconnected: reconnected when
   * they drop or the machine wakes up, and at the next launch.
   */
  wanted: Record<string, true>
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
  /** Tabs with a statement in flight, by tab id: when it started (ms epoch). */
  running: Record<string, number>
  /** Tabs whose last statement failed, by tab id, until their next run. */
  failed: Record<string, { error: string; detail?: string }>
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
  wanted: {},
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
  failed: {},
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

/** Keeps new ids clear of restored ones (state/session.ts): they start at `from` or later. */
export function reserveIds(from: number) {
  nextId = Math.max(nextId, from)
}

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

/** Every shown database of every connection, in sidebar order. */
export function allDatabases(): Active[] {
  const s = useStore.getState()
  const scopes = usePrefs.getState().scopes
  return s.connectionOrder.flatMap((id) => {
    const c = s.connections[id]
    return c ? shownDatabases(scopes, c).map((d) => ({ connectionId: id, database: d })) : []
  })
}

// ── Scope (what the explorer shows and introspects) ───────────────────────

export type Scope = string[] | '*'
type Scopes = Record<string, Scope>

export const schemaScopeKey = (connectionId: string, database: string) =>
  `${connectionId}/${database}`

/** The connection's picked databases, in server order (default: its own database). */
export function shownDatabases(scopes: Scopes, c: Connection): string[] {
  const names = c.databases.map((d) => d.name)
  const sel = scopes[c.id]
  if (sel === '*') return names
  const want = new Set(sel ?? [c.database])
  return names.filter((n) => want.has(n))
}

/** The database's picked schemas, in server order (default: `public`, else the first). */
export function databaseSchemas(
  scopes: Scopes,
  connectionId: string,
  database: string,
  names: string[]
): string[] {
  const sel = scopes[schemaScopeKey(connectionId, database)]
  if (sel === '*') return names
  if (sel) {
    const want = new Set(sel)
    return names.filter((n) => want.has(n))
  }
  return names.includes('public') ? ['public'] : names.slice(0, 1)
}

/** Saves a picker choice and loads what became visible. */
export function setScope(key: string, scope: Scope) {
  setPrefs({ scopes: { ...usePrefs.getState().scopes, [key]: scope } })
  const slash = key.indexOf('/')
  if (slash >= 0) {
    void loadDatabase(key.slice(0, slash), key.slice(slash + 1))
    return
  }
  const c = useStore.getState().connections[key]
  if (!c) return
  const expanded = usePrefs.getState().treeExpanded
  for (const d of shownDatabases(usePrefs.getState().scopes, c))
    if (expanded[schemaScopeKey(key, d)]) void loadDatabase(key, d)
}

// ── Selection and tabs ────────────────────────────────────────────────────

function patchWorkspace(key: string, f: (ws: Workspace) => Workspace) {
  useStore.setState((s) => ({
    workspaces: { ...s.workspaces, [key]: f(s.workspaces[key] ?? { tabs: [], activeTab: null }) }
  }))
}

/** Selects a database, connecting its connection if it isn't (as DataGrip does). */
export function selectDatabase(connectionId: string, database: string) {
  useStore.setState({ active: { connectionId, database } })
  const state = useStore.getState().connections[connectionId]?.state
  if (state === 'disconnected' || state === 'failed')
    void ensureConnected(connectionId).then((ok) => {
      if (ok) void loadDatabase(connectionId, database)
    })
  else void loadDatabase(connectionId, database)
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

/** Sets a table tab's WHERE / ORDER BY; an empty clause is removed. */
export function setTableQuery(key: string, id: string, q: { where?: string; orderBy?: string }) {
  patchWorkspace(key, (ws) => ({
    ...ws,
    tabs: ws.tabs.map((t) => {
      if (t.id !== id || t.kind !== 'table') return t
      const { where: _w, orderBy: _o, ...rest } = t
      const where = q.where?.trim()
      const orderBy = q.orderBy?.trim()
      return { ...rest, ...(where ? { where } : {}), ...(orderBy ? { orderBy } : {}) }
    })
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

/** Asked before a tab closes; returning false keeps it open (the guard may close it later). */
let closeGuard: ((key: string, id: string) => boolean) | null = null
export function setCloseGuard(g: (key: string, id: string) => boolean) {
  closeGuard = g
}

export function closeTab(key: string, id: string, { force = false }: { force?: boolean } = {}) {
  if (!force && closeGuard && !closeGuard(key, id)) return
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
    readOnly: c.readOnly ?? false,
    environment: c.environment ?? undefined,
    color: c.color ?? undefined,
    ssh: c.ssh ?? undefined,
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
    sslMode: c.sslMode,
    readOnly: c.readOnly,
    environment: c.environment,
    color: c.color,
    ssh: c.ssh
  }
}

/** Loads the saved connections (once, at startup); false if that failed. */
export async function loadConnections(): Promise<boolean> {
  try {
    const list = await backend.listConnections()
    useStore.setState((s) => ({
      connections: Object.fromEntries(list.map((c) => [c.id, fromSaved(c, s.connections[c.id])])),
      connectionOrder: list.map((c) => c.id)
    }))
    return true
  } catch (e) {
    reportError('Could not load connections', e)
    return false
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
  // Reconnecting rolls back open transactions: ask first (declined: the
  // new settings apply at the next connect).
  if (
    prev?.state === 'connected' &&
    (await confirmCloseTx('reconnect', { [config.id]: config.name }, config.id))
  ) {
    stopRecovery(config.id)
    await connectNow(config.id, savePassword ? undefined : (password ?? undefined))
  }
}

export async function deleteConnection(id: string) {
  await backend.deleteConnection(id)
  forgetHistory(id)
  stopRecovery(id)
  useStore.setState((s) => {
    const { [id]: _, ...connections } = s.connections
    const { [id]: _w, ...wanted } = s.wanted
    const workspaces = Object.fromEntries(
      Object.entries(s.workspaces).filter(([k]) => !k.startsWith(`${id}\u0000`))
    )
    return {
      connections,
      wanted,
      workspaces,
      connectionOrder: s.connectionOrder.filter((x) => x !== id),
      active: s.active?.connectionId === id ? null : s.active
    }
  })
}

type ConnectResult = { ok: true } | { ok: false; error: BackendError }

/** Connect attempts in flight: a second caller waits for the same one. */
const connecting = new Map<string, Promise<ConnectResult>>()

/**
 * Connects (with `password`, else the stored one), lists the databases and
 * loads the default one. A server asking for a password that isn't stored
 * opens the password prompt (not when `quiet`); other errors show a toast
 * (not when `toast: false`).
 */
function connectNow(
  id: string,
  password?: string,
  opts: { quiet?: boolean; toast?: boolean } = {}
): Promise<ConnectResult> {
  const pending = connecting.get(id)
  if (pending) return pending
  const conn = useStore.getState().connections[id]
  if (!conn) return Promise.resolve({ ok: false, error: { message: 'Unknown connection' } })
  const run = (async (): Promise<ConnectResult> => {
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
      useStore.setState((s) => ({
        wanted: { ...s.wanted, [id]: true },
        passwordPrompt: s.passwordPrompt?.connectionId === id ? null : s.passwordPrompt
      }))
      // The first shown database, plus any shown ones left expanded.
      const shown = shownDatabases(usePrefs.getState().scopes, useStore.getState().connections[id]!)
      const expanded = usePrefs.getState().treeExpanded
      shown.forEach((name, i) => {
        if (i === 0 || expanded[schemaScopeKey(id, name)]) void loadDatabase(id, name)
      })
      return { ok: true }
    } catch (e) {
      const err = toBackendError(e)
      const auth = isAuthError(err)
      patchConnection(id, { state: auth ? 'disconnected' : 'failed' })
      if (auth && opts.quiet) {
        // Restoring at launch: don't ask for passwords of the connections in the background.
      } else if (auth && (password != null || !conn.hasPassword || err.code === '28P01')) {
        useStore.setState({
          passwordPrompt: {
            connectionId: id,
            error: password != null || err.code === '28P01' ? err.message : undefined
          }
        })
      } else if (opts.toast !== false) {
        pushToast({ kind: 'error', title: `Could not connect to ${conn.name}`, body: err.message })
      }
      return { ok: false, error: err }
    }
  })()
  connecting.set(id, run)
  void run.finally(() => connecting.delete(id))
  return run
}

const isAuthError = (e: BackendError) =>
  e.code === 'password_required' || e.code === '28P01' || e.code === '28000'

/** Worth trying again: the server couldn't be reached (not a refused login or a bad setting). */
const isRetryable = (e: BackendError) => !e.code || e.code === 'unreachable'

/**
 * Connects (with `password`, else the stored one). While the connection is
 * being re-established after a drop, retries that right away instead.
 */
export async function connect(id: string, password?: string): Promise<boolean> {
  if (password == null && recoveries.has(id)) return retryNow(id)
  stopRecovery(id)
  return (await connectNow(id, password)).ok
}

/**
 * Makes sure the connection is open before acting on it: connects it, waits
 * for a connect in progress, or retries a reconnect now. False if it stays closed.
 */
export async function ensureConnected(id: string): Promise<boolean> {
  const c = useStore.getState().connections[id]
  if (!c) return false
  if (c.state === 'connected') return true
  const pending = connecting.get(id)
  if (pending) return (await pending).ok
  return connect(id)
}

export async function disconnect(id: string) {
  const name = useStore.getState().connections[id]?.name ?? id
  if (!(await confirmCloseTx('disconnect', { [id]: name }, id))) return
  stopRecovery(id)
  unwant(id)
  patchConnection(id, { state: 'disconnected', latencyMs: null })
  try {
    await backend.disconnect(id)
  } catch (e) {
    reportError('Disconnect failed', e)
  }
}

function unwant(id: string) {
  useStore.setState((s) => {
    if (!s.wanted[id]) return {}
    const { [id]: _, ...wanted } = s.wanted
    return { wanted }
  })
}

// ── Staying connected ─────────────────────────────────────────────────────
// A wanted connection that drops is brought back: `recover` checks it with
// `ping` (which keeps the sessions that still work, and their
// transactions), or connects it again if the backend no longer has it (a
// new launch), retrying with growing delays while the server can't be
// reached. Meanwhile it shows as `reconnecting`; after the last attempt as
// `failed`, with a Retry toast. Waking from sleep and the network coming
// back check every wanted connection.

/** Delays before each retry; once they run out the connection shows as failed. */
export const RETRY_DELAYS_MS = [1_000, 2_000, 4_000, 8_000, 15_000, 30_000, 30_000, 30_000]

/** Why a connection is being recovered: a call failed, the machine woke up, or the app launched. */
type RecoverReason = 'lost' | 'wake' | 'restore'

type Recovery = {
  reason: RecoverReason
  /** Don't ask for a password (connections restored in the background). */
  quiet: boolean
  tries: number
  timer?: ReturnType<typeof setTimeout>
  attempt?: Promise<boolean>
  /** It showed as reconnecting (so getting back is worth a toast). */
  flagged: boolean
  done: Promise<boolean>
  resolve: (ok: boolean) => void
}

const recoveries = new Map<string, Recovery>()

/**
 * Brings a wanted connection back (see above); resolves once it is
 * connected (true) or given up on (false). Joins a recovery in progress.
 */
export function recover(
  id: string,
  { reason = 'lost', quiet = false }: { reason?: RecoverReason; quiet?: boolean } = {}
): Promise<boolean> {
  const running = recoveries.get(id)
  if (running) return running.done
  let resolve!: (ok: boolean) => void
  const done = new Promise<boolean>((r) => {
    resolve = r
  })
  recoveries.set(id, { reason, quiet, tries: 0, flagged: false, done, resolve })
  void step(id)
  return done
}

/** Runs the pending attempt of a recovery now; true if it got the connection back. */
export async function retryNow(id: string): Promise<boolean> {
  const r = recoveries.get(id)
  if (!r) return useStore.getState().connections[id]?.state === 'connected'
  return step(id)
}

/** Ends a recovery (the user connected, disconnected or deleted the connection). */
function stopRecovery(id: string) {
  const r = recoveries.get(id)
  if (!r) return
  clearTimeout(r.timer)
  recoveries.delete(id)
  r.resolve(useStore.getState().connections[id]?.state === 'connected')
}

async function step(id: string): Promise<boolean> {
  const r = recoveries.get(id)
  if (!r) return false
  if (r.attempt) return r.attempt
  clearTimeout(r.timer)
  r.timer = undefined
  r.attempt = attemptRecovery(id, r)
  try {
    return await r.attempt
  } finally {
    r.attempt = undefined
  }
}

async function attemptRecovery(id: string, r: Recovery): Promise<boolean> {
  const name = () => useStore.getState().connections[id]?.name ?? id
  const started = performance.now()
  let error: BackendError
  try {
    const info = await backend.ping(id)
    if (recoveries.get(id) !== r) return false
    noteLostTransactions(id, info.lostTx)
    patchConnection(id, {
      state: 'connected',
      latencyMs: Math.round(performance.now() - started)
    })
    finish(id, r, true)
    if (r.flagged && r.reason !== 'restore')
      pushToast({ kind: 'success', title: `Reconnected to ${name()}` })
    return true
  } catch (e) {
    error = toBackendError(e)
  }
  if (recoveries.get(id) !== r) return false
  if (error.code === 'not_connected') {
    // The backend doesn't have it open (a new launch, or it was never connected).
    const res = await connectNow(id, undefined, { quiet: r.quiet, toast: false })
    if (recoveries.get(id) !== r) return res.ok
    if (res.ok) {
      finish(id, r, true)
      if (r.flagged && r.reason !== 'restore')
        pushToast({ kind: 'success', title: `Reconnected to ${name()}` })
      return true
    }
    error = res.error
    if (!isRetryable(error)) {
      // A refused login (the prompt is up, or skipped in the background) or a bad setting.
      if (isAuthError(error)) {
        if (r.quiet) unwant(id)
      } else
        pushToast({ kind: 'error', title: `Could not connect to ${name()}`, body: error.message })
      finish(id, r, false)
      return false
    }
  }
  const delay = RETRY_DELAYS_MS[r.tries++]
  if (delay === undefined) {
    patchConnection(id, { state: 'failed', latencyMs: null })
    finish(id, r, false)
    pushToast(
      {
        kind: 'error',
        title:
          r.reason === 'restore'
            ? `Could not connect to ${name()}`
            : `Lost connection to ${name()}`,
        body: error.message,
        action: { label: 'Retry', run: () => void recover(id) }
      },
      0
    )
    return false
  }
  r.flagged = true
  patchConnection(id, { state: 'reconnecting', latencyMs: null })
  r.timer = setTimeout(() => void step(id), delay)
  return false
}

function finish(id: string, r: Recovery, ok: boolean) {
  clearTimeout(r.timer)
  if (recoveries.get(id) === r) recoveries.delete(id)
  r.resolve(ok)
}

/** Warns about transactions the server rolled back when their session dropped. */
function noteLostTransactions(id: string, databases: string[]) {
  const name = useStore.getState().connections[id]?.name ?? id
  for (const database of databases) {
    noteTx(id, database, { state: 'idle' })
    pushToast(
      {
        kind: 'error',
        title: 'Transaction rolled back',
        body: `The connection to ${name} (${database}) was lost, so the server rolled back the open transaction. Uncommitted changes are gone.`
      },
      0
    )
  }
}

onLinkEvent((e) => {
  if (e.kind === 'txLost') return noteLostTransactions(e.connectionId, [e.database])
  if (useStore.getState().connections[e.connectionId]?.state === 'connected')
    void recover(e.connectionId, { reason: 'lost' })
})

/**
 * Checks every wanted connection (after sleep, or when the network comes
 * back): open ones are pinged, ones being retried are retried now, failed
 * ones get another round.
 */
export function checkConnections() {
  const s = useStore.getState()
  for (const id of Object.keys(s.wanted)) {
    const state = s.connections[id]?.state
    if (recoveries.has(id)) void retryNow(id)
    else if (state === 'connected') void recover(id, { reason: 'wake' })
    else if (state === 'failed' || state === 'reconnecting') void recover(id, { reason: 'lost' })
  }
}

/** Timers stop while the machine sleeps: a tick this late means it woke up. */
const WAKE_TICK_MS = 5_000
const WAKE_GAP_MS = 20_000
let watching = false

/** Starts checking connections on wake and when the network comes back (once). */
export function watchConnections() {
  if (watching || typeof window === 'undefined') return
  watching = true
  window.addEventListener('online', checkConnections)
  let last = Date.now()
  setInterval(() => {
    const now = Date.now()
    if (now - last > WAKE_GAP_MS) checkConnections()
    last = now
  }, WAKE_TICK_MS)
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

/**
 * Lists the database's schema names, then introspects the picked schemas
 * that are not loaded yet and drops the unpicked ones (`force`: re-reads
 * everything). Re-runs if the pick changed meanwhile.
 */
export async function loadDatabase(id: string, database: string, { force = false } = {}) {
  const dbNow = () =>
    useStore.getState().connections[id]?.databases.find((d) => d.name === database)
  const c = useStore.getState().connections[id]
  const db = dbNow()
  if (c?.state !== 'connected' || !db || db.loading) return
  patchDatabase(id, database, { loading: true })
  let want: string[] = []
  try {
    let names = db.schemaNames
    if (!names || force) {
      names = await backend.listSchemas(id, database)
      patchDatabase(id, database, { schemaNames: names })
    }
    want = databaseSchemas(usePrefs.getState().scopes, id, database, names)
    const kept = force ? [] : (dbNow()?.schemas ?? []).filter((s) => want.includes(s.name))
    const missing = want.filter((n) => !kept.some((s) => s.name === n))
    const fetched = missing.length ? await backend.introspect(id, database, missing) : []
    const byName = new Map([...kept, ...fetched].map((s) => [s.name, s]))
    const schemas = names.flatMap((n) => byName.get(n) ?? [])
    patchDatabase(id, database, { schemas, loaded: true, loading: false })
  } catch (e) {
    patchDatabase(id, database, { loading: false })
    reportError(`Could not read ${database}`, e)
    return
  }
  // The pick may have changed while we were loading.
  const after = dbNow()
  if (!after?.schemaNames) return
  const now = databaseSchemas(usePrefs.getState().scopes, id, database, after.schemaNames)
  if (now.join('\u0000') !== want.join('\u0000')) void loadDatabase(id, database)
}

export function connectionString(c: Connection, database?: string): string {
  const auth = c.user ? `${encodeURIComponent(c.user)}@` : ''
  const port = c.port ? `:${c.port}` : ''
  return `postgresql://${auth}${c.host}${port}/${database ?? c.database}`
}

// ── Queries ───────────────────────────────────────────────────────────────

/** Adds to the Output log; `record`: also to the saved query history (not for page fetches). */
function log(connectionId: string, entry: HistoryEntry, record = true) {
  useStore.setState((s) => ({
    history: {
      ...s.history,
      [connectionId]: [entry, ...(s.history[connectionId] ?? [])].slice(0, 200)
    }
  }))
  if (record) recordHistory(connectionId, entry)
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
  const failed = (err: { error: string; detail?: string }) =>
    useStore.setState((s) => ({
      failed: { ...s.failed, [tabId]: err },
      bottomOpen: true,
      bottomView: 'output'
    }))
  useStore.setState((s) => {
    if (!s.failed[tabId]) return {}
    const { [tabId]: _, ...rest } = s.failed
    return { failed: rest }
  })
  // Not connected: connect first (or wait for the reconnect), then run.
  if (useStore.getState().connections[connectionId]?.state !== 'connected')
    await ensureConnected(connectionId)
  const conn = useStore.getState().connections[connectionId]
  const at = Date.now()
  const base = { sql: text, database, at, source: title }
  if (conn?.state !== 'connected') {
    const error = `${conn?.name ?? 'Connection'} is not connected`
    log(connectionId, { ...base, id: nextId++, ms: 0, rows: null, error })
    failed({ error })
    return
  }
  // Guard rails: no way out of read-only, and a confirmation before
  // destructive statements (lib/guard.ts, state/guards.ts).
  const writable = conn.readOnly ? readOnlyEscape(text) : null
  if (writable) {
    log(connectionId, { ...base, id: nextId++, ms: 0, rows: null, error: `Not run: ${writable}` })
    failed({ error: `Not run: ${writable}` })
    return
  }
  if (!(await confirmRun(conn, database, text))) return
  useStore.setState((s) => ({ running: { ...s.running, [tabId]: Date.now() } }))
  try {
    const pageSize = usePrefs.getState().rowLimit
    const res = await backend.runQuery(connectionId, database, text, pageSize)
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
        connectionId,
        database,
        columns: res.columns,
        rows: res.rows,
        truncated: res.truncated,
        pageable: res.pageable,
        offset: 0,
        pageSize,
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
    const err = errorText(e)
    log(connectionId, { ...base, id: nextId++, ms: Date.now() - at, rows: null, ...err })
    failed(err)
    if (toBackendError(e).code === 'not_connected')
      patchConnection(connectionId, { state: 'disconnected' })
  } finally {
    useStore.setState((s) => {
      const { [tabId]: _, ...running } = s.running
      return { running }
    })
  }
}

/** Adds a run that didn't go through `runQuery` (grid edits) to the Output log. */
export function logRun(connectionId: string, entry: Omit<HistoryEntry, 'id'>) {
  log(connectionId, { ...entry, id: nextId++ })
}

/** Asks the server to cancel the tab's running statement (it then fails with 57014). */
export function cancelQuery(tabId: string) {
  const { running, active } = useStore.getState()
  if (!running[tabId] || !active) return
  void backend
    .cancelQuery(active.connectionId, active.database)
    .catch((e) => reportError('Cancel failed', e))
}

/**
 * Shows another page of a tab's result by re-running its statement with an
 * offset (DataGrip's LIMIT/OFFSET paging: stateless, so pages reflect the
 * data at the time each one is fetched).
 */
export async function pageResult(tabId: string, offset: number) {
  const r = useStore.getState().results[tabId]
  if (!r?.pageable || useStore.getState().running[tabId]) return
  const at = Date.now()
  const base = { sql: r.sql, database: r.database, at, source: r.title }
  useStore.setState((s) => ({ running: { ...s.running, [tabId]: at } }))
  try {
    const res = await backend.runQuery(r.connectionId, r.database, r.sql, r.pageSize, offset)
    log(
      r.connectionId,
      {
        ...base,
        id: nextId++,
        ms: res.ms,
        rows: res.rows.length,
        truncated: res.truncated,
        offset
      },
      false
    )
    useStore.setState((s) =>
      // Closed or re-run meanwhile: drop this page.
      s.results[tabId]?.at !== r.at
        ? {}
        : {
            results: {
              ...s.results,
              [tabId]: {
                ...r,
                columns: res.columns,
                rows: res.rows,
                truncated: res.truncated,
                offset,
                ms: res.ms
              }
            }
          }
    )
  } catch (e) {
    const err = errorText(e)
    log(r.connectionId, { ...base, id: nextId++, ms: Date.now() - at, rows: null, ...err }, false)
    pushToast({ kind: 'error', title: 'Could not load page', body: err.error })
    if (toBackendError(e).code === 'not_connected')
      patchConnection(r.connectionId, { state: 'disconnected' })
  } finally {
    useStore.setState((s) => {
      const { [tabId]: _, ...running } = s.running
      return { running }
    })
  }
}

/** Adds an Output log entry for work outside `runQuery` (CSV exports). */
export function logOutput(connectionId: string, entry: Omit<HistoryEntry, 'id'>) {
  log(connectionId, { ...entry, id: nextId++ })
}

/** `[SQLSTATE] message` and the detail / hint lines of a backend error. */
export const describeError = errorText

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

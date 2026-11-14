// The database backend the store talks to. In the Tauri app these are the
// Rust commands (src/lib.rs: saved connections, Postgres sessions,
// introspection, queries). In a plain browser (`pnpm dev:web`) a mock with
// the same shape serves the sample data in state/mock.ts, so the UI can be
// worked on without a server.

import { invoke, isTauri } from '@tauri-apps/api/core'
import { execute } from '@/state/mockData'
import { sampleConnections } from '@/state/mock'
import { type ExportBackend, mockExport, tauriExport } from './exportBackend'
import type { ResultColumn, Schema } from '@/state/types'
import type { EditStatement } from './gridEdits'
import { applyMockEdits } from '@/state/mockEdits'
import { localHistoryStore, type HistoryRecord } from './history'

export type SslMode = 'disable' | 'prefer' | 'require' | 'verify-full'

export type ConnConfig = {
  id: string
  name: string
  /** Host name, IP, or a Unix socket directory (starting with `/`). */
  host: string
  port: number
  user: string
  /** Database connected to first; the others are listed from it. */
  database: string
  sslMode: SslMode
}

export type SavedConnection = ConnConfig & { hasPassword: boolean }

export type ConnectInfo = { serverVersion: string; databases: string[] }

export type QueryResult = {
  columns: ResultColumn[]
  rows: (string | null)[][]
  /** Rows affected / returned, as the server reported it. */
  affected: number | null
  /** More rows existed after this page. */
  truncated: boolean
  /** Read-only statement: other pages can be fetched by re-running it with an offset. */
  pageable: boolean
  ms: number
}

/** What every backend call rejects with (Postgres details when the server raised it). */
export type BackendError = {
  message: string
  /** SQLSTATE (`42P01`) or an app code: `password_required`, `not_connected`. */
  code?: string
  /** 1-based character offset into the statement. */
  position?: number
  detail?: string
  hint?: string
}

export function toBackendError(e: unknown): BackendError {
  if (e && typeof e === 'object' && 'message' in e) return e as BackendError
  return { message: String(e) }
}

/** Result of submitting grid edits (`applyEdits`). */
export type ApplyOutcome = {
  /** Statements that ran, in order (rolled back again if one failed). */
  runs: { affected: number; ms: number }[]
  /** The statement that failed; everything was rolled back. */
  failed: { index: number; error: BackendError } | null
  /** Ran inside the user's open transaction (under a savepoint, not committed). */
  inTransaction: boolean
  ms: number
}

export type Backend = {
  /** False for the in-browser mock. */
  real: boolean
  listConnections(): Promise<SavedConnection[]>
  /** `password`: a new one to store (when `savePassword`), or null to keep the stored one. */
  saveConnection(config: ConnConfig, password: string | null, savePassword: boolean): Promise<void>
  deleteConnection(id: string): Promise<void>
  /** Connects with these settings and disconnects; resolves to the server version. */
  testConnection(config: ConnConfig, password: string | null): Promise<string>
  connect(id: string, password?: string): Promise<ConnectInfo>
  disconnect(id: string): Promise<void>
  /** Every schema name in the database (cheap; for the schema picker). */
  listSchemas(id: string, database: string): Promise<string[]>
  /** Tables, views and columns of the given schemas only. */
  introspect(id: string, database: string, schemas: string[]): Promise<Schema[]>
  /** Runs `sql`, returning `limit` rows after the first `offset` (pageable statements only). */
  runQuery(
    id: string,
    database: string,
    sql: string,
    limit: number,
    offset?: number
  ): Promise<QueryResult>
  cancelQuery(id: string, database: string): Promise<void>
  /** The connection's saved query history, oldest first. */
  historyList(id: string): Promise<HistoryRecord[]>
  historyAppend(id: string, entry: HistoryRecord): Promise<void>
  historyDelete(id: string, ids: string[]): Promise<void>
  historyClear(id: string): Promise<void>
  /**
   * Runs the data editor's statements as one unit: all or nothing, each
   * `exactlyOne` statement must affect exactly one row.
   */
  applyEdits(id: string, database: string, statements: EditStatement[]): Promise<ApplyOutcome>
} & ExportBackend

const tauriBackend: Backend = {
  real: true,
  listConnections: () => invoke('list_connections'),
  saveConnection: (config, password, savePassword) =>
    invoke('save_connection', { config, password, savePassword }),
  deleteConnection: (id) => invoke('delete_connection', { id }),
  testConnection: (config, password) => invoke('test_connection', { config, password }),
  connect: (id, password) => invoke('connect', { id, password: password ?? null }),
  disconnect: (id) => invoke('disconnect', { id }),
  listSchemas: (id, database) => invoke('list_schemas', { id, database }),
  introspect: (id, database, schemas) => invoke('introspect', { id, database, schemas }),
  runQuery: (id, database, sql, limit, offset = 0) =>
    invoke('run_query', { id, database, sql, limit, offset }),
  cancelQuery: (id, database) => invoke('cancel_query', { id, database }),
  ...tauriExport,
  applyEdits: (id, database, statements) => invoke('apply_edits', { id, database, statements }),
  historyList: (id) => invoke('history_list', { id }),
  historyAppend: (id, entry) => invoke('history_append', { id, entry }),
  historyDelete: (id, ids) => invoke('history_delete', { id, ids }),
  historyClear: (id) => invoke('history_clear', { id })
}

// ── In-browser mock ───────────────────────────────────────────────────────

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

function mockBackend(): Backend {
  const samples = sampleConnections()
  const saved = new Map<string, SavedConnection>(
    samples.map((c) => [
      c.id,
      {
        id: c.id,
        name: c.name,
        host: c.host,
        port: c.port ?? 5432,
        user: c.user ?? 'postgres',
        database: c.databases[0]?.name ?? 'postgres',
        sslMode: 'prefer',
        hasPassword: true
      }
    ])
  )
  const dbOf = (id: string, database: string) =>
    samples.find((c) => c.id === id)?.databases.find((d) => d.name === database)
  const cancelled = new Set<string>()
  const history = localHistoryStore(typeof localStorage === 'undefined' ? undefined : localStorage)
  return {
    real: false,
    listConnections: async () => [...saved.values()],
    saveConnection: async (config, password, savePassword) => {
      const prev = saved.get(config.id)
      saved.set(config.id, {
        ...config,
        hasPassword: savePassword && (password != null || !!prev?.hasPassword)
      })
    },
    deleteConnection: async (id) => {
      saved.delete(id)
      history.clear(id)
    },
    testConnection: async (config) => {
      await sleep(300)
      if (!config.host) throw { message: 'Host is empty' }
      return 'PostgreSQL 17.2 (mock)'
    },
    connect: async (id) => {
      await sleep(400)
      const sample = samples.find((c) => c.id === id)
      if (!sample) throw { message: 'No server here: the browser preview only has sample data' }
      return {
        serverVersion: sample.serverVersion ?? 'PostgreSQL 17.2',
        databases: sample.databases.map((d) => d.name)
      }
    },
    disconnect: async () => {},
    listSchemas: async (id, database) => {
      await sleep(80)
      return dbOf(id, database)?.schemas.map((s) => s.name) ?? []
    },
    introspect: async (id, database, schemas) => {
      await sleep(150)
      return dbOf(id, database)?.schemas.filter((s) => schemas.includes(s.name)) ?? []
    },
    runQuery: async (id, database, sql, limit, offset = 0) => {
      const key = `${id}\u0000${database}`
      cancelled.delete(key)
      const ms = 20 + Math.round(Math.random() * 400)
      await sleep(ms)
      if (cancelled.has(key))
        throw { message: 'canceling statement due to user request', code: '57014' }
      const res = execute(dbOf(id, database), sql, limit, offset)
      if (res.kind === 'error') throw { message: res.message, code: '42P01' }
      if (res.kind === 'affected') {
        if (offset) throw { message: "This statement can't be paged" }
        return { columns: [], rows: [], affected: res.count, truncated: false, pageable: false, ms }
      }
      return {
        columns: res.columns,
        rows: res.rows,
        affected: res.rows.length,
        truncated: res.truncated ?? false,
        pageable: true,
        ms
      }
    },
    cancelQuery: async (id, database) => {
      cancelled.add(`${id}\u0000${database}`)
    },
    ...mockExport(dbOf, cancelled),
    applyEdits: async (id, database, statements) => {
      await sleep(150)
      return applyMockEdits(dbOf(id, database), statements)
    },
    historyList: async (id) => history.list(id),
    historyAppend: async (id, entry) => history.append(id, entry),
    historyDelete: async (id, ids) => history.remove(id, ids),
    historyClear: async (id) => history.clear(id)
  }
}

export const backend: Backend = isTauri() ? tauriBackend : mockBackend()

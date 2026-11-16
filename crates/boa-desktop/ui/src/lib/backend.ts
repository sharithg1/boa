// The database backend the store talks to. In the Tauri app these are the
// Rust commands (src/lib.rs: saved connections, Postgres sessions,
// introspection, queries). In a plain browser (`pnpm dev:web`) a mock with
// the same shape serves the sample data in state/mock.ts, so the UI can be
// worked on without a server.

import { invoke, isTauri } from '@tauri-apps/api/core'
import { execute } from '@/state/mockData'
import { sampleConnections } from '@/state/mock'
import { type ExportBackend, mockExport, tauriExport } from './exportBackend'
import type { EditStatement } from './gridEdits'
import { applyMockEdits } from '@/state/mockEdits'
import { localHistoryStore, type HistoryRecord } from './history'
import type { Environment, ResultColumn, Schema } from '@/state/types'
import { trackTx, type TxState } from '@/state/tx'
import { ddlFromMetadata } from './tableMeta'

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
  /** Sessions run with `default_transaction_read_only = on`. */
  readOnly: boolean
  environment?: Environment
  /** Palette colour name (lib/connTag.ts). */
  color?: string
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
  /** The session's transaction state after the statement. */
  tx?: TxState
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
  /** The session's transaction state after a failed statement. */
  tx?: TxState
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
  /** The session's transaction state afterwards (Manual mode opens one). */
  tx?: TxState
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
  /** Tables, views, columns, indexes and constraints of the given schemas only. */
  introspect(id: string, database: string, schemas: string[]): Promise<Schema[]>
  /** DDL of one table / view, as the server describes it (CREATE, indexes, triggers, comments). */
  tableDdl(id: string, database: string, schema: string, table: string): Promise<string>
  /** Runs `sql`, returning `limit` rows after the first `offset` (pageable statements only). */
  runQuery(
    id: string,
    database: string,
    sql: string,
    limit: number,
    offset?: number
  ): Promise<QueryResult>
  cancelQuery(id: string, database: string): Promise<void>
  /** Manual (a transaction opens before the first statement) or Auto commit; resolves to the state. */
  setTxMode(id: string, database: string, manual: boolean): Promise<TxState>
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
  tableDdl: (id, database, schema, table) => invoke('table_ddl', { id, database, schema, table }),
  runQuery: (id, database, sql, limit, offset = 0) =>
    invoke('run_query', { id, database, sql, limit, offset }),
  cancelQuery: (id, database) => invoke('cancel_query', { id, database }),
  ...tauriExport,
  applyEdits: (id, database, statements) => invoke('apply_edits', { id, database, statements }),
  historyList: (id) => invoke('history_list', { id }),
  historyAppend: (id, entry) => invoke('history_append', { id, entry }),
  historyDelete: (id, ids) => invoke('history_delete', { id, ids }),
  historyClear: (id) => invoke('history_clear', { id }),
  setTxMode: (id, database, manual) => invoke('set_tx_mode', { id, database, manual })
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
        hasPassword: true,
        readOnly: false,
        // Tagged so the browser preview shows the production guard rails.
        environment: c.id === 'prod-pg' ? ('production' as const) : undefined
      }
    ])
  )
  const dbOf = (id: string, database: string) =>
    samples.find((c) => c.id === id)?.databases.find((d) => d.name === database)
  const cancelled = new Set<string>()
  const history = localHistoryStore(typeof localStorage === 'undefined' ? undefined : localStorage)
  // Simplified transactions: begin / commit / rollback by first keyword,
  // Manual mode, aborted blocks (25P02) and read-only connections (25006).
  const sessions = new Map<string, { manual: boolean; tx: TxState }>()
  const session = (key: string) => {
    let s = sessions.get(key)
    if (!s) sessions.set(key, (s = { manual: false, tx: 'idle' }))
    return s
  }
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
    disconnect: async (id) => {
      for (const k of sessions.keys()) if (k.startsWith(`${id}\u0000`)) sessions.delete(k)
    },
    listSchemas: async (id, database) => {
      await sleep(80)
      return dbOf(id, database)?.schemas.map((s) => s.name) ?? []
    },
    introspect: async (id, database, schemas) => {
      await sleep(150)
      return dbOf(id, database)?.schemas.filter((s) => schemas.includes(s.name)) ?? []
    },
    tableDdl: async (id, database, schema, table) => {
      await sleep(60)
      const t = dbOf(id, database)
        ?.schemas.find((s) => s.name === schema)
        ?.tables.find((x) => x.name === table)
      if (!t) throw { message: `${schema}.${table} not found` }
      return ddlFromMetadata(schema, t)
    },
    runQuery: async (id, database, sql, limit, offset = 0) => {
      const key = `${id}\u0000${database}`
      cancelled.delete(key)
      const ms = 20 + Math.round(Math.random() * 400)
      await sleep(ms)
      const s = session(key)
      const fail = (e: BackendError): never => {
        if (s.tx !== 'idle') s.tx = 'failed'
        throw { ...e, tx: s.tx }
      }
      if (cancelled.has(key))
        fail({ message: 'canceling statement due to user request', code: '57014' })
      const kw = /^[\s(]*([a-z]+)/i.exec(sql.replace(/^\s*--.*$/gm, ''))?.[1]?.toLowerCase() ?? ''
      const done = { columns: [], rows: [], affected: 0, truncated: false, pageable: false, ms }
      if (['commit', 'end', 'rollback', 'abort'].includes(kw)) {
        s.tx = 'idle'
        return { ...done, tx: s.tx }
      }
      if (s.tx === 'failed')
        fail({
          message:
            'current transaction is aborted, commands ignored until end of transaction block',
          code: '25P02'
        })
      if (kw === 'begin' || kw === 'start') {
        s.tx = 'active'
        return { ...done, tx: s.tx }
      }
      if (s.manual && s.tx === 'idle' && !offset) s.tx = 'active'
      if (
        saved.get(id)?.readOnly &&
        /^(insert|update|delete|merge|create|drop|alter|truncate)$/.test(kw)
      )
        fail({
          message: `cannot execute ${kw.toUpperCase()} in a read-only transaction`,
          code: '25006',
          hint: 'The connection is read-only (data source settings).'
        })
      const res = execute(dbOf(id, database), sql, limit, offset)
      if (res.kind === 'error') return fail({ message: res.message, code: '42P01' })
      if (res.kind === 'affected') {
        if (offset) throw { message: "This statement can't be paged", tx: s.tx }
        return { ...done, affected: res.count, tx: s.tx }
      }
      return {
        columns: res.columns,
        rows: res.rows,
        affected: res.rows.length,
        truncated: res.truncated ?? false,
        pageable: true,
        ms,
        tx: s.tx
      }
    },
    cancelQuery: async (id, database) => {
      cancelled.add(`${id}\u0000${database}`)
    },
    ...mockExport(dbOf, cancelled),
    applyEdits: async (id, database, statements) => {
      await sleep(150)
      const s = session(`${id}\u0000${database}`)
      if (s.tx === 'failed')
        return {
          runs: [],
          failed: {
            index: 0,
            error: {
              code: '25P02',
              message:
                'current transaction is aborted, commands ignored until end of transaction block'
            }
          },
          inTransaction: true,
          tx: s.tx,
          ms: 0
        }
      // Manual mode opens a transaction first, as a typed statement would.
      if (s.manual && s.tx === 'idle') s.tx = 'active'
      const out = applyMockEdits(dbOf(id, database), statements)
      return { ...out, inTransaction: s.tx === 'active', tx: s.tx }
    },
    historyList: async (id) => history.list(id),
    historyAppend: async (id, entry) => history.append(id, entry),
    historyDelete: async (id, ids) => history.remove(id, ids),
    historyClear: async (id) => history.clear(id),
    setTxMode: async (id, database, manual) => {
      const s = session(`${id}\u0000${database}`)
      s.manual = manual
      return s.tx
    }
  }
}

export const backend: Backend = trackTx(isTauri() ? tauriBackend : mockBackend())

// The database backend the store talks to. In the Tauri app these are the
// Rust commands (src/lib.rs: saved connections, Postgres sessions,
// introspection, queries). In a plain browser (`pnpm dev:web`) a mock with
// the same shape serves the sample data in state/mock.ts, so the UI can be
// worked on without a server.

import { invoke, isTauri } from '@tauri-apps/api/core'
import { execute } from '@/state/mockData'
import { sampleConnections } from '@/state/mock'
import type { ResultColumn, Schema } from '@/state/types'

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
  /** More rows existed than the limit. */
  truncated: boolean
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
  introspect(id: string, database: string): Promise<Schema[]>
  runQuery(id: string, database: string, sql: string, limit: number): Promise<QueryResult>
  cancelQuery(id: string, database: string): Promise<void>
}

const tauriBackend: Backend = {
  real: true,
  listConnections: () => invoke('list_connections'),
  saveConnection: (config, password, savePassword) =>
    invoke('save_connection', { config, password, savePassword }),
  deleteConnection: (id) => invoke('delete_connection', { id }),
  testConnection: (config, password) => invoke('test_connection', { config, password }),
  connect: (id, password) => invoke('connect', { id, password: password ?? null }),
  disconnect: (id) => invoke('disconnect', { id }),
  introspect: (id, database) => invoke('introspect', { id, database }),
  runQuery: (id, database, sql, limit) => invoke('run_query', { id, database, sql, limit }),
  cancelQuery: (id, database) => invoke('cancel_query', { id, database })
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
    introspect: async (id, database) => {
      await sleep(150)
      return dbOf(id, database)?.schemas ?? []
    },
    runQuery: async (id, database, sql, limit) => {
      const key = `${id}\u0000${database}`
      cancelled.delete(key)
      const ms = 20 + Math.round(Math.random() * 400)
      await sleep(ms)
      if (cancelled.has(key))
        throw { message: 'canceling statement due to user request', code: '57014' }
      const res = execute(dbOf(id, database), sql, limit)
      if (res.kind === 'error') throw { message: res.message, code: '42P01' }
      if (res.kind === 'affected')
        return { columns: [], rows: [], affected: res.count, truncated: false, ms }
      return {
        columns: res.columns,
        rows: res.rows,
        affected: res.rows.length,
        truncated: false,
        ms
      }
    },
    cancelQuery: async (id, database) => {
      cancelled.add(`${id}\u0000${database}`)
    }
  }
}

export const backend: Backend = isTauri() ? tauriBackend : mockBackend()

// Shapes the UI works with. Schemas / tables / columns come from the Rust
// side's `introspect` command (lib/backend.ts), or state/mock.ts in a browser.

export type Engine = 'postgres' | 'mysql' | 'sqlite'

export type ConnState = 'connected' | 'connecting' | 'disconnected' | 'failed'

export type Column = {
  name: string
  type: string
  nullable: boolean
  pk: boolean
  default: string | null
}

export type Table = {
  name: string
  kind: 'table' | 'view'
  /** Approximate row count (planner estimate). */
  rows: number
  sizeBytes: number
  columns: Column[]
}

export type Schema = { name: string; tables: Table[] }

export type Database = {
  name: string
  schemas: Schema[]
  /** Introspected (schemas are filled in); databases load on first expand / select. */
  loaded?: boolean
  loading?: boolean
}

export type SslMode = 'disable' | 'prefer' | 'require' | 'verify-full'

export type Connection = {
  id: string
  name: string
  engine: Engine
  /** Hostname, or the file path for SQLite. */
  host: string
  port: number | null
  user: string | null
  state: ConnState
  latencyMs: number | null
  serverVersion: string | null
  databases: Database[]
  /** Database connected to first. */
  database: string
  sslMode: SslMode
  /** A password is stored in the OS keychain. */
  hasPassword: boolean
}

export type HistoryEntry = {
  id: number
  sql: string
  database: string
  at: number
  ms: number
  rows: number | null
  /** More rows existed than were fetched. */
  truncated?: boolean
  /** Server detail / hint lines for errors. */
  detail?: string
  /** Affected rows, for statements that return none. */
  affected?: number
  /** Editor tab title it ran from (`console_1`). */
  source?: string
  error?: string
}

export type ResultColumn = { name: string; type: string }

/** One executed statement's rows, shown in the Database Output grid. */
export type ResultSet = {
  /** Editor tab that produced it. */
  tabId: string
  /** Label of the result tab: `db.schema.table` or the console name. */
  title: string
  sql: string
  columns: ResultColumn[]
  rows: (string | null)[][]
  /** More rows existed than the row limit. */
  truncated?: boolean
  ms: number
  at: number
}

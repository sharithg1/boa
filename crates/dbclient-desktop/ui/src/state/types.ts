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
  /** Every schema name (listed cheaply); `schemas` holds only the picked ones, introspected. */
  schemaNames?: string[]
  /** Introspected (schemas are filled in); databases load on first expand / select. */
  loaded?: boolean
  loading?: boolean
}

export type SslMode = 'disable' | 'prefer' | 'require' | 'verify-full'

/** Environment tag of a connection (production always confirms destructive statements). */
export type Environment = 'development' | 'test' | 'staging' | 'production'

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
  /** A password is stored in the connections config file. */
  hasPassword: boolean
  /** Sessions are read-only on the server (`default_transaction_read_only`). */
  readOnly: boolean
  environment?: Environment
  /** Palette colour name (lib/connTag.ts); unset: the environment's colour. */
  color?: string
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
  /** Rows skipped before the fetched page. */
  offset?: number
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
  connectionId: string
  database: string
  columns: ResultColumn[]
  rows: (string | null)[][]
  /** More rows exist after this page. */
  truncated?: boolean
  /** Read-only statement: other pages re-run it with LIMIT/OFFSET semantics. */
  pageable: boolean
  /** Rows skipped before this page. */
  offset: number
  /** Page size it was first run with (the row limit then). */
  pageSize: number
  ms: number
  at: number
}

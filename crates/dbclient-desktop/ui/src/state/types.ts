// Shapes the UI works with. The Rust side will return these from its
// connection / introspection commands; today they come from state/mock.ts.

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

export type Database = { name: string; schemas: Schema[] }

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
}

export type HistoryEntry = {
  id: number
  sql: string
  database: string
  at: number
  ms: number
  rows: number | null
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
  ms: number
  at: number
}

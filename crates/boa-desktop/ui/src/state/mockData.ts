// Simulated execution until the Rust side has drivers: deterministic sample
// rows per table, and a tiny SQL front end that understands enough of
// `SELECT … FROM [schema.]table [WHERE …] [ORDER BY …] [LIMIT n]` to return
// those rows (state/mockQuery.ts evaluates simple conditions). Anything
// else "succeeds" with an affected-rows count.
import type { Column, Database, ResultColumn, Table } from './types'
import { editedRows } from './mockEdits'
import { filterAndSort, tailClauses } from './mockQuery'

/** Sample rows a filtered or sorted mock query looks at (tables claim millions). */
const MOCK_SCAN_ROWS = 20_000

function hash(s: string): number {
  let h = 2166136261
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619)
  return h >>> 0
}

/** mulberry32: small seeded PRNG so a table shows the same rows every time. */
function rng(seed: number) {
  let a = seed
  return () => {
    a = (a + 0x6d2b79f5) | 0
    let t = Math.imul(a ^ (a >>> 15), 1 | a)
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

const FIRST = ['ada', 'grace', 'linus', 'barbara', 'ken', 'margaret', 'alan', 'edsger', 'donald']
const LAST = ['lovelace', 'hopper', 'torvalds', 'liskov', 'thompson', 'hamilton', 'turing', 'knuth']
const WORDS = ['alpha', 'bravo', 'delta', 'echo', 'kilo', 'lima', 'oscar', 'sierra', 'tango']
const ENUMS: Record<string, string[]> = {
  role: ['owner', 'admin', 'member', 'member', 'viewer'],
  status: ['paid', 'open', 'void', 'paid', 'uncollectible'],
  plan: ['free', 'pro', 'team', 'enterprise'],
  currency: ['USD', 'EUR', 'GBP'],
  action: ['login', 'logout', 'invoice.paid', 'user.created', 'org.updated']
}

function pick<T>(r: () => number, xs: readonly T[]): T {
  return xs[Math.floor(r() * xs.length)]!
}

const pad = (n: number, w = 2) => String(n).padStart(w, '0')

function timestamp(r: () => number, withZone: boolean): string {
  const d = new Date(Date.UTC(2026, 10, 12) - Math.floor(r() * 400 * 86400_000))
  const base = `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())} ${pad(
    Math.floor(r() * 24)
  )}:${pad(Math.floor(r() * 60))}:${pad(Math.floor(r() * 60))}`
  return withZone ? `${base}.${pad(Math.floor(r() * 1e6), 6)} +00:00` : base
}

function value(c: Column, i: number, r: () => number): string | null {
  const name = c.name.toLowerCase()
  const type = c.type.toLowerCase()
  if (c.nullable && r() < 0.15) return null
  if (c.pk && /int|serial/.test(type)) return String(i + 1)
  if (type === 'uuid')
    return [8, 4, 4, 4, 12]
      .map((n) => Array.from({ length: n }, () => Math.floor(r() * 16).toString(16)).join(''))
      .join('-')
  if (name.includes('email')) return `${pick(r, FIRST)}.${pick(r, LAST)}${i}@example.com`
  if (name === 'name' || name === 'title' || name === 'label')
    return `${pick(r, FIRST)} ${pick(r, LAST)}`.replace(/\b\w/g, (m) => m.toUpperCase())
  for (const [k, vs] of Object.entries(ENUMS)) if (name.includes(k)) return pick(r, vs)
  if (name === 'path') return `/${pick(r, WORDS)}/${pick(r, WORDS)}`
  if (name === 'referrer') return `https://${pick(r, WORDS)}.example.com/`
  if (name === 'sku') return `SKU-${Math.floor(r() * 1e6)}`
  if (type.startsWith('timestamp') || type === 'datetime')
    return timestamp(r, type === 'timestamptz')
  if (type === 'date') return timestamp(r, false).slice(0, 10)
  if (type === 'jsonb' || type === 'json')
    return `{"ip": "10.0.${i % 255}.${Math.floor(r() * 255)}"}`
  if (type.startsWith('decimal') || name.includes('price') || name.includes('total'))
    return (r() * 500).toFixed(2)
  if (name.endsWith('_id')) return String(1 + Math.floor(r() * 9000))
  if (name === 'done') return r() < 0.5 ? '0' : '1'
  if (/int|bigint|serial/.test(type)) return String(Math.floor(r() * 100_000))
  return `${pick(r, WORDS)}_${pick(r, WORDS)}`
}

/** The first `limit` rows of a table (at most its row estimate). */
export function sampleRows(table: Table, limit: number): (string | null)[][] {
  const r = rng(hash(table.name))
  const n = Math.min(limit, table.rows)
  return Array.from({ length: n }, (_, i) => table.columns.map((c) => value(c, i, r)))
}

export const tableColumns = (t: Table): ResultColumn[] =>
  t.columns.map((c) => ({ name: c.name, type: c.type }))

// ── Statements ────────────────────────────────────────────────────────────

/** Statement under the caret: the `;`-separated chunk containing `pos`. */
export function statementAt(sql: string, pos: number): string {
  let start = 0
  for (const part of sql.split(';')) {
    const end = start + part.length
    if (pos <= end && part.trim()) return part.trim()
    start = end + 1
  }
  // Caret after the last `;`: run the last non-empty statement.
  const parts = sql.split(';').filter((p) => p.trim())
  return parts[parts.length - 1]?.trim() ?? ''
}

export type Execution =
  | {
      kind: 'rows'
      columns: ResultColumn[]
      rows: (string | null)[][]
      /** More rows exist after this page. */
      truncated?: boolean
      table?: string
    }
  | { kind: 'affected'; count: number }
  | { kind: 'error'; message: string; code?: string }

const unquote = (s: string) => s.replace(/^["`]|["`]$/g, '')

export function execute(
  db: Database | undefined,
  sql: string,
  rowLimit: number,
  offset = 0
): Execution {
  const text = sql.trim().replace(/;$/, '')
  if (!/^(select|with|table|show|explain|values)\b/i.test(text)) {
    if (
      !/^(insert|update|delete|create|drop|alter|truncate|begin|commit|rollback|set)\b/i.test(text)
    )
      return { kind: 'error', message: `syntax error at or near "${text.split(/\s+/)[0]}"` }
    return {
      kind: 'affected',
      count: /^(insert|update|delete)/i.test(text) ? 1 + Math.floor(Math.random() * 20) : 0
    }
  }
  const from = /\bfrom\s+((?:"[^"]+"|`[^`]+`|\w+)(?:\.(?:"[^"]+"|`[^`]+`|\w+))?)/i.exec(text)
  if (!from) {
    // SELECT without FROM: one row, one column per expression.
    const list = text
      .replace(/^select\s+/i, '')
      .split(',')
      .map((s) => s.trim())
      .filter(Boolean)
    return {
      kind: 'rows',
      columns: list.map((e) => {
        const as = /\s+as\s+("?)(\w+)\1$/i.exec(e)
        return { name: as ? as[2]! : '?column?', type: /^\d+$/.test(e) ? 'integer' : 'text' }
      }),
      rows: [list.map((e) => e.replace(/\s+as\s+.*$/i, '').replace(/^'(.*)'$/, '$1'))]
    }
  }
  const parts = from[1]!.split('.').map(unquote)
  const [schemaName, tableName] = parts.length === 2 ? parts : [undefined, parts[0]!]
  const schemas = db?.schemas.filter((s) => !schemaName || s.name === schemaName) ?? []
  let table: Table | undefined
  let schema = ''
  for (const s of schemas) {
    table = s.tables.find((t) => t.name === tableName)
    if (table) {
      schema = s.name
      break
    }
  }
  if (!table) return { kind: 'error', message: `relation "${parts.join('.')}" does not exist` }
  const limit = /\blimit\s+(\d+)/i.exec(text)
  // Rows submitted from the data editor (state/mockEdits.ts) replace the samples.
  let source = editedRows(db?.name ?? '', schema, table.name)
  let count = source?.length ?? table.rows
  const clauses = tailClauses(text.slice(from.index + from[0].length))
  if (clauses.where || clauses.orderBy) {
    // Filtered / sorted: over the first MOCK_SCAN_ROWS sample rows only.
    const all = source ?? sampleRows(table, Math.min(count, MOCK_SCAN_ROWS))
    const res = filterAndSort(tableColumns(table), all, clauses)
    if (res.kind === 'error') return res
    source = res.rows
    count = source.length
  }
  const total = Math.min(limit ? Number(limit[1]) : count, count)
  const end = Math.min(total, offset + rowLimit)
  let columns = tableColumns(table)
  let rows = (source ?? sampleRows(table, end)).slice(offset, end)
  const list = /^select\s+(.*?)\s+from\b/is.exec(text)?.[1]?.trim()
  if (list && list !== '*') {
    const wanted = list.split(',').map((s) => unquote(s.trim().split('.').pop()!))
    const idx = wanted.map((w) => columns.findIndex((c) => c.name === w))
    const bad = wanted.find((_, i) => idx[i]! < 0)
    if (bad) return { kind: 'error', message: `column "${bad}" does not exist` }
    columns = idx.map((i) => columns[i]!)
    rows = rows.map((row) => idx.map((i) => row[i]!))
  }
  return {
    kind: 'rows',
    columns,
    rows,
    truncated: end < total,
    table: `${db?.name}.${schema}.${table.name}`
  }
}

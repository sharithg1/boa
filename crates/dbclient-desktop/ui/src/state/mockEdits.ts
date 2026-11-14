// Grid edits in the browser mock: an edited table's rows (its first 10,000
// generated ones) are kept with the changes applied, so a reload shows them
// until the page is refreshed. Mirrors the real command's rules: all or
// nothing, exactly one row per UPDATE / DELETE, primary keys unique, NOT
// NULL columns need a value or a default.
import type { ApplyOutcome } from '@/lib/backend'
import type { EditStatement, RowChange, Value } from '@/lib/gridEdits'
import { sampleRows } from './mockData'
import type { Database, Table } from './types'

const edited = new Map<string, Value[][]>()
/** An edited table keeps only this many of its generated rows (some have millions). */
const MAX_ROWS = 10_000
const keyOf = (db: string, schema: string, table: string) => `${db}\u0000${schema}\u0000${table}`

/** The table's rows with submitted edits applied, or null if it was never edited. */
export function editedRows(db: string, schema: string, table: string): Value[][] | null {
  return edited.get(keyOf(db, schema, table)) ?? null
}

type Fail = { message: string; code: string }

function matches(table: Table, row: Value[], key: Record<string, Value>): boolean {
  return Object.entries(key).every(
    ([k, v]) => row[table.columns.findIndex((c) => c.name === k)] === v
  )
}

function defaultFor(table: Table, i: number, rows: Value[][]): Value {
  const c = table.columns[i]!
  if (c.pk && /int|serial/i.test(c.type))
    return String(rows.reduce((m, r) => Math.max(m, Number(r[i]) || 0), 0) + 1)
  if (c.default && /now\(\)|current_timestamp/i.test(c.default))
    return new Date().toISOString().replace('T', ' ').replace('Z', '+00')
  if (c.default) return c.default.replace(/^'(.*)'(::.*)?$/, '$1')
  return null
}

function check(table: Table, rows: Value[][], row: Value[], skip: number): Fail | null {
  for (const [i, c] of table.columns.entries())
    if (!c.nullable && row[i] === null)
      return {
        code: '23502',
        message: `null value in column "${c.name}" of relation "${table.name}" violates not-null constraint`
      }
  const pk = table.columns.flatMap((c, i) => (c.pk ? [i] : []))
  if (pk.length && rows.some((r, j) => j !== skip && pk.every((i) => r[i] === row[i])))
    return {
      code: '23505',
      message: `duplicate key value violates unique constraint "${table.name}_pkey"`
    }
  return null
}

/** Applies one change to `rows` in place; returns the affected count or a failure. */
function applyOne(table: Table, rows: Value[][], change: RowChange): number | Fail {
  const col = (name: string) => table.columns.findIndex((c) => c.name === name)
  if (change.kind === 'insert') {
    const row = table.columns.map((c, i) =>
      c.name in change.values ? change.values[c.name]! : defaultFor(table, i, rows)
    )
    const bad = check(table, rows, row, -1)
    if (bad) return bad
    rows.push(row)
    return 1
  }
  const hits = rows.flatMap((r, i) => (matches(table, r, change.key) ? [i] : []))
  if (hits.length !== 1) return hits.length
  const at = hits[0]!
  if (change.kind === 'delete') {
    rows.splice(at, 1)
    return 1
  }
  const row = [...rows[at]!]
  for (const [k, v] of Object.entries(change.values)) {
    const i = col(k)
    if (i < 0) return { code: '42703', message: `column "${k}" does not exist` }
    row[i] = v
  }
  const bad = check(table, rows, row, at)
  if (bad) return bad
  rows[at] = row
  return 1
}

export function applyMockEdits(
  db: Database | undefined,
  statements: EditStatement[]
): ApplyOutcome {
  const started = performance.now()
  const work = new Map<string, Value[][]>()
  const runs: ApplyOutcome['runs'] = []
  for (const [index, st] of statements.entries()) {
    const table = db?.schemas
      .find((s) => s.name === st.schema)
      ?.tables.find((t) => t.name === st.table)
    const fail = (error: Fail | { message: string; code?: string }) => ({
      runs,
      failed: { index, error },
      inTransaction: false,
      ms: Math.round(performance.now() - started)
    })
    if (!table || !db)
      return fail({ code: '42P01', message: `relation "${st.schema}.${st.table}" does not exist` })
    const key = keyOf(db.name, st.schema, st.table)
    let rows = work.get(key)
    if (!rows) {
      rows = (edited.get(key) ?? sampleRows(table, Math.min(table.rows, MAX_ROWS))).map((r) => [
        ...r
      ])
      work.set(key, rows)
    }
    const r = applyOne(table, rows, st.change)
    if (typeof r !== 'number') return fail(r)
    if (st.exactlyOne && r !== 1) {
      const verb = { update: 'updated', delete: 'deleted', insert: 'inserted' }[st.change.kind]
      return fail({
        code: 'row_count',
        message: `Expected 1 row to be ${verb}, but ${r} were`
      })
    }
    runs.push({ affected: r, ms: 1 })
  }
  for (const [k, rows] of work) edited.set(k, rows)
  return { runs, failed: null, inTransaction: false, ms: Math.round(performance.now() - started) }
}

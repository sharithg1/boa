// Pending edits of a table data tab (DataGrip's staged changes) and the SQL
// that applies them. Pure: the grid keeps a `Pending` per tab, these
// functions update it, and `buildStatements` turns it into one statement per
// changed row:
//
//   UPDATE t SET c = '…' WHERE pk = '<original>'   edited row
//   INSERT INTO t (c, …) VALUES ('…', …)            new row (untouched columns omitted)
//   DELETE FROM t WHERE pk = '<original>'           deleted row
//
// Values are the server's text rendering, so they go out as quoted string
// literals and Postgres coerces them to the column type; NULL stays bare.
// The WHERE always uses the row's original key values, so editing a key
// column still finds the row.
import { qualified, quoteIdent } from './sql'
import type { ResultColumn, Table } from '@/state/types'

export type Value = string | null

/** A row of the grid: a loaded row (index into the page's rows) or a new one. */
export type RowRef = { kind: 'base'; index: number } | { kind: 'new'; id: number }

export type NewRow = { id: number; values: Record<number, Value> }

export type Pending = {
  /** Loaded row index → column index → new value. */
  updates: Record<number, Record<number, Value>>
  /** Loaded row indices marked for deletion. */
  deletes: Record<number, true>
  /** Rows to insert; only the columns set here are inserted. */
  inserts: NewRow[]
  /** Next new-row id. */
  nextId: number
}

export const EMPTY_PENDING: Pending = { updates: {}, deletes: {}, inserts: [], nextId: 1 }

export function changeCount(p: Pending): number {
  const updated = Object.keys(p.updates).filter((r) => !p.deletes[Number(r)]).length
  return updated + Object.keys(p.deletes).length + p.inserts.length
}

export const isEmpty = (p: Pending) => changeCount(p) === 0

/** Sets a cell. Setting a loaded cell back to its original value drops the edit. */
export function setCell(
  p: Pending,
  base: readonly (readonly Value[])[],
  ref: RowRef,
  col: number,
  value: Value
): Pending {
  if (ref.kind === 'new') {
    return {
      ...p,
      inserts: p.inserts.map((r) =>
        r.id === ref.id ? { ...r, values: { ...r.values, [col]: value } } : r
      )
    }
  }
  const original = base[ref.index]?.[col]
  if (original === undefined || p.deletes[ref.index]) return p
  const row = { ...p.updates[ref.index] }
  if (value === original) delete row[col]
  else row[col] = value
  const updates = { ...p.updates }
  if (Object.keys(row).length) updates[ref.index] = row
  else delete updates[ref.index]
  return { ...p, updates }
}

/** Undoes one cell: a loaded cell gets its original value, a new row's cell its default. */
export function revertCell(p: Pending, ref: RowRef, col: number): Pending {
  if (ref.kind === 'new') {
    return {
      ...p,
      inserts: p.inserts.map((r) => {
        if (r.id !== ref.id) return r
        const { [col]: _, ...values } = r.values
        return { ...r, values }
      })
    }
  }
  const row = p.updates[ref.index]
  if (!row || !(col in row)) return p
  const { [col]: _, ...rest } = row
  const updates = { ...p.updates }
  if (Object.keys(rest).length) updates[ref.index] = rest
  else delete updates[ref.index]
  return { ...p, updates }
}

/** Undoes everything on a row: edits and deletion, or drops a new row. */
export function revertRow(p: Pending, ref: RowRef): Pending {
  if (ref.kind === 'new') return { ...p, inserts: p.inserts.filter((r) => r.id !== ref.id) }
  const { [ref.index]: _u, ...updates } = p.updates
  const { [ref.index]: _d, ...deletes } = p.deletes
  return { ...p, updates, deletes }
}

/** Marks a loaded row for deletion (its edits are dropped); a new row just goes away. */
export function deleteRow(p: Pending, ref: RowRef): Pending {
  if (ref.kind === 'new') return revertRow(p, ref)
  const { [ref.index]: _, ...updates } = p.updates
  return { ...p, updates, deletes: { ...p.deletes, [ref.index]: true } }
}

export function addRow(p: Pending): { pending: Pending; id: number } {
  const id = p.nextId
  return { pending: { ...p, inserts: [...p.inserts, { id, values: {} }], nextId: id + 1 }, id }
}

export type RowMark = 'inserted' | 'deleted' | 'updated' | undefined

export function rowMark(p: Pending, ref: RowRef): RowMark {
  if (ref.kind === 'new') return 'inserted'
  if (p.deletes[ref.index]) return 'deleted'
  return p.updates[ref.index] ? 'updated' : undefined
}

export function cellEdited(p: Pending, ref: RowRef, col: number): boolean {
  if (ref.kind === 'new') return col in (p.inserts.find((r) => r.id === ref.id)?.values ?? {})
  return !p.deletes[ref.index] && col in (p.updates[ref.index] ?? {})
}

// ── Editability ───────────────────────────────────────────────────────────

export type Editability = { editable: true; pk: string[] } | { editable: false; reason: string }

/** Whether a table tab's rows can be edited: a table with a primary key, all of it loaded. */
export function editability(
  table: Table | undefined,
  columns: readonly ResultColumn[]
): Editability {
  if (!table) return { editable: false, reason: 'Read-only: table not introspected' }
  if (table.kind === 'view') return { editable: false, reason: 'Read-only: views are not editable' }
  const pk = table.columns.filter((c) => c.pk).map((c) => c.name)
  if (!pk.length) return { editable: false, reason: 'Read-only: the table has no primary key' }
  const names = columns.map((c) => c.name)
  if (pk.some((k) => names.filter((n) => n === k).length !== 1))
    return { editable: false, reason: 'Read-only: the primary key is not in the result' }
  return { editable: true, pk }
}

// ── SQL ───────────────────────────────────────────────────────────────────

/**
 * A string literal for `v`, or NULL. Backslashes switch to an E'' literal
 * (with `\\`), which reads the same whatever `standard_conforming_strings` is.
 */
export function sqlLiteral(v: Value): string {
  if (v === null) return 'NULL'
  const quoted = v.replace(/'/g, "''")
  return v.includes('\\') ? `E'${quoted.replace(/\\/g, '\\\\')}'` : `'${quoted}'`
}

export type EditTarget = {
  schema: string
  table: string
  /** Result columns, in grid order. */
  columns: readonly string[]
  /** Primary-key column names. */
  pk: readonly string[]
}

/** What a statement does, by column name (the browser mock applies these). */
export type RowChange =
  | { kind: 'update'; key: Record<string, Value>; values: Record<string, Value> }
  | { kind: 'insert'; values: Record<string, Value> }
  | { kind: 'delete'; key: Record<string, Value> }

export type EditStatement = {
  sql: string
  schema: string
  table: string
  /** Must affect exactly one row, or the whole submit rolls back. */
  exactlyOne: boolean
  change: RowChange
}

function keyOf(target: EditTarget, row: readonly Value[]): Record<string, Value> {
  const key: Record<string, Value> = {}
  for (const k of target.pk) {
    const i = target.columns.indexOf(k)
    if (i < 0) throw new Error(`Primary key column ${k} is not in the result`)
    key[k] = row[i] ?? null
  }
  return key
}

const where = (key: Record<string, Value>) =>
  Object.entries(key)
    .map(([k, v]) =>
      v === null ? `${quoteIdent(k)} IS NULL` : `${quoteIdent(k)} = ${sqlLiteral(v)}`
    )
    .join(' AND ')

/**
 * The statements that apply `p` to the loaded `base` rows: deletes first
 * (so a new row may reuse a deleted key), then updates, then inserts, each
 * group in row order.
 */
export function buildStatements(
  target: EditTarget,
  base: readonly (readonly Value[])[],
  p: Pending
): EditStatement[] {
  if (!target.pk.length) throw new Error('A table without a primary key cannot be edited')
  const table = qualified(target.schema, target.table)
  const out: EditStatement[] = []
  const rowsOf = (o: Record<number, unknown>) =>
    Object.keys(o)
      .map(Number)
      .sort((a, b) => a - b)

  for (const r of rowsOf(p.deletes)) {
    const row = base[r]
    if (!row) continue
    const key = keyOf(target, row)
    out.push({
      sql: `DELETE FROM ${table} WHERE ${where(key)}`,
      exactlyOne: true,
      schema: target.schema,
      table: target.table,
      change: { kind: 'delete', key }
    })
  }

  for (const r of rowsOf(p.updates)) {
    const row = base[r]
    if (!row || p.deletes[r]) continue
    const cols = rowsOf(p.updates[r]!).filter(
      (c) => c < target.columns.length && p.updates[r]![c] !== row[c]
    )
    if (!cols.length) continue
    const key = keyOf(target, row)
    const values: Record<string, Value> = {}
    for (const c of cols) values[target.columns[c]!] = p.updates[r]![c] ?? null
    const set = Object.entries(values)
      .map(([k, v]) => `${quoteIdent(k)} = ${sqlLiteral(v)}`)
      .join(', ')
    out.push({
      sql: `UPDATE ${table} SET ${set} WHERE ${where(key)}`,
      exactlyOne: true,
      schema: target.schema,
      table: target.table,
      change: { kind: 'update', key, values }
    })
  }

  for (const n of p.inserts) {
    const cols = rowsOf(n.values).filter((c) => c < target.columns.length)
    const values: Record<string, Value> = {}
    for (const c of cols) values[target.columns[c]!] = n.values[c] ?? null
    const names = Object.keys(values)
    out.push({
      sql: names.length
        ? `INSERT INTO ${table} (${names.map(quoteIdent).join(', ')}) VALUES (${names
            .map((k) => sqlLiteral(values[k] ?? null))
            .join(', ')})`
        : `INSERT INTO ${table} DEFAULT VALUES`,
      exactlyOne: true,
      schema: target.schema,
      table: target.table,
      change: { kind: 'insert', values }
    })
  }
  return out
}

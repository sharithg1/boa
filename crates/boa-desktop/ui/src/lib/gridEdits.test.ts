import { describe, expect, it } from 'vitest'
import {
  EMPTY_PENDING,
  addRow,
  buildStatements,
  cellEdited,
  changeCount,
  deleteRow,
  editability,
  revertCell,
  revertRow,
  rowMark,
  setCell,
  sqlLiteral,
  type EditTarget,
  type Pending,
  type Value
} from './gridEdits'
import type { Column, Table } from '@/state/types'

const base: Value[][] = [
  ['1', 'ada', 'a@x.io'],
  ['2', "o'brien", null],
  ['3', 'grace', 'g@x.io']
]
const target: EditTarget = {
  schema: 'public',
  table: 'users',
  columns: ['id', 'name', 'email'],
  pk: ['id']
}
const b = (index: number) => ({ kind: 'base' as const, index })
const sqls = (p: Pending, t = target, rows = base) => buildStatements(t, rows, p).map((s) => s.sql)

describe('sqlLiteral', () => {
  it('quotes and doubles single quotes', () => {
    expect(sqlLiteral('abc')).toBe("'abc'")
    expect(sqlLiteral("it's")).toBe("'it''s'")
    expect(sqlLiteral('')).toBe("''")
    expect(sqlLiteral("''")).toBe("''''''")
  })
  it('leaves NULL bare and distinct from the text "NULL"', () => {
    expect(sqlLiteral(null)).toBe('NULL')
    expect(sqlLiteral('NULL')).toBe("'NULL'")
  })
  it('uses an E-literal with doubled backslashes when there are backslashes', () => {
    expect(sqlLiteral('C:\\dir')).toBe("E'C:\\\\dir'")
    expect(sqlLiteral("a\\'b")).toBe("E'a\\\\''b'")
  })
  it('keeps newlines, unicode and semicolons inside the literal', () => {
    expect(sqlLiteral('a\nb; drop table x; --')).toBe("'a\nb; drop table x; --'")
    expect(sqlLiteral('héllo ✓')).toBe("'héllo ✓'")
  })
})

describe('pending state', () => {
  it('records an edit and drops it when set back to the original', () => {
    let p = setCell(EMPTY_PENDING, base, b(0), 1, 'lovelace')
    expect(p.updates).toEqual({ 0: { 1: 'lovelace' } })
    expect(cellEdited(p, b(0), 1)).toBe(true)
    expect(rowMark(p, b(0))).toBe('updated')
    p = setCell(p, base, b(0), 1, 'ada')
    expect(p.updates).toEqual({})
    expect(changeCount(p)).toBe(0)
  })

  it('treats NULL and empty string as different values', () => {
    const p = setCell(EMPTY_PENDING, base, b(1), 2, '')
    expect(p.updates).toEqual({ 1: { 2: '' } })
    expect(setCell(p, base, b(1), 2, null).updates).toEqual({})
    expect(setCell(EMPTY_PENDING, base, b(0), 2, null).updates).toEqual({ 0: { 2: null } })
  })

  it('reverts a cell and a row', () => {
    let p = setCell(EMPTY_PENDING, base, b(0), 1, 'x')
    p = setCell(p, base, b(0), 2, 'y')
    expect(revertCell(p, b(0), 1).updates).toEqual({ 0: { 2: 'y' } })
    expect(revertCell(revertCell(p, b(0), 1), b(0), 2).updates).toEqual({})
    expect(revertRow(deleteRow(p, b(0)), b(0))).toMatchObject({ updates: {}, deletes: {} })
  })

  it('deleting a row drops its edits and blocks new ones', () => {
    let p = setCell(EMPTY_PENDING, base, b(2), 1, 'x')
    p = deleteRow(p, b(2))
    expect(p.updates).toEqual({})
    expect(rowMark(p, b(2))).toBe('deleted')
    expect(setCell(p, base, b(2), 1, 'y')).toBe(p)
    expect(changeCount(p)).toBe(1)
  })

  it('adds, edits, reverts and deletes new rows', () => {
    const { pending, id } = addRow(EMPTY_PENDING)
    const ref = { kind: 'new' as const, id }
    let p = setCell(pending, base, ref, 1, 'new')
    expect(p.inserts).toEqual([{ id, values: { 1: 'new' } }])
    expect(cellEdited(p, ref, 1)).toBe(true)
    expect(cellEdited(p, ref, 2)).toBe(false)
    expect(rowMark(p, ref)).toBe('inserted')
    expect(revertCell(p, ref, 1).inserts).toEqual([{ id, values: {} }])
    expect(changeCount(p)).toBe(1)
    p = deleteRow(p, ref)
    expect(p.inserts).toEqual([])
    expect(addRow(p).id).toBe(id + 1)
  })
})

describe('buildStatements', () => {
  it('emits nothing for no changes', () => {
    expect(sqls(EMPTY_PENDING)).toEqual([])
  })

  it('updates only the changed columns, keyed by the primary key', () => {
    let p = setCell(EMPTY_PENDING, base, b(1), 1, 'obrien')
    p = setCell(p, base, b(1), 2, "o'b@x.io")
    expect(sqls(p)).toEqual([
      `UPDATE public.users SET name = 'obrien', email = 'o''b@x.io' WHERE id = '2'`
    ])
    const [s] = buildStatements(target, base, p)
    expect(s!.exactlyOne).toBe(true)
    expect(s!.change).toEqual({
      kind: 'update',
      key: { id: '2' },
      values: { name: 'obrien', email: "o'b@x.io" }
    })
  })

  it('sets NULL unquoted and empty strings as quotes', () => {
    let p = setCell(EMPTY_PENDING, base, b(0), 2, null)
    p = setCell(p, base, b(2), 2, '')
    expect(sqls(p)).toEqual([
      `UPDATE public.users SET email = NULL WHERE id = '1'`,
      `UPDATE public.users SET email = '' WHERE id = '3'`
    ])
  })

  it('uses the original key in the WHERE when the key itself is edited', () => {
    const p = setCell(EMPTY_PENDING, base, b(0), 0, '10')
    expect(sqls(p)).toEqual([`UPDATE public.users SET id = '10' WHERE id = '1'`])
  })

  it('handles composite keys, quoted identifiers and key edits', () => {
    const t: EditTarget = {
      schema: 'Sales',
      table: 'order items',
      columns: ['order_id', 'Line', 'qty', 'select'],
      pk: ['order_id', 'Line']
    }
    const rows: Value[][] = [['7', '1', '3', 'x']]
    let p = setCell(EMPTY_PENDING, rows, b(0), 1, '2')
    p = setCell(p, rows, b(0), 3, 'y')
    expect(sqls(p, t, rows)).toEqual([
      `UPDATE "Sales"."order items" SET "Line" = '2', "select" = 'y' WHERE order_id = '7' AND "Line" = '1'`
    ])
    expect(sqls(deleteRow(EMPTY_PENDING, b(0)), t, rows)).toEqual([
      `DELETE FROM "Sales"."order items" WHERE order_id = '7' AND "Line" = '1'`
    ])
  })

  it('inserts only the columns that were set, or DEFAULT VALUES', () => {
    const a = addRow(EMPTY_PENDING)
    const c = addRow(a.pending)
    let p = setCell(c.pending, base, { kind: 'new', id: a.id }, 2, 'x@y.z')
    p = setCell(p, base, { kind: 'new', id: a.id }, 1, null)
    expect(sqls(p)).toEqual([
      `INSERT INTO public.users (name, email) VALUES (NULL, 'x@y.z')`,
      'INSERT INTO public.users DEFAULT VALUES'
    ])
    expect(buildStatements(target, base, p)[1]!.change).toEqual({ kind: 'insert', values: {} })
  })

  it('orders deletes, then updates, then inserts', () => {
    let p = setCell(EMPTY_PENDING, base, b(2), 1, 'g')
    p = deleteRow(p, b(1))
    const n = addRow(p)
    p = setCell(n.pending, base, { kind: 'new', id: n.id }, 0, '2')
    p = setCell(p, base, b(0), 1, 'a')
    expect(sqls(p)).toEqual([
      `DELETE FROM public.users WHERE id = '2'`,
      `UPDATE public.users SET name = 'a' WHERE id = '1'`,
      `UPDATE public.users SET name = 'g' WHERE id = '3'`,
      `INSERT INTO public.users (id) VALUES ('2')`
    ])
  })

  it('skips no-op updates and rows missing from the page', () => {
    const p: Pending = {
      ...EMPTY_PENDING,
      updates: { 0: { 1: 'ada' }, 9: { 1: 'x' } },
      deletes: { 9: true }
    }
    expect(sqls(p)).toEqual([])
  })

  it('escapes values that look like SQL', () => {
    const p = setCell(EMPTY_PENDING, base, b(0), 1, "'); drop table users; --")
    expect(sqls(p)).toEqual([
      `UPDATE public.users SET name = '''); drop table users; --' WHERE id = '1'`
    ])
  })

  it('refuses a target without a primary key', () => {
    expect(() => buildStatements({ ...target, pk: [] }, base, EMPTY_PENDING)).toThrow()
  })
})

describe('editability', () => {
  const col = (name: string, pk = false): Column => ({
    name,
    type: 'text',
    nullable: true,
    pk,
    default: null
  })
  const table = (columns: Column[], kind: Table['kind'] = 'table'): Table => ({
    name: 't',
    kind,
    rows: 0,
    sizeBytes: 0,
    columns
  })
  const rc = (...names: string[]) => names.map((name) => ({ name, type: 'text' }))

  it('allows tables with a primary key in the result', () => {
    expect(editability(table([col('id', true), col('v')]), rc('id', 'v'))).toEqual({
      editable: true,
      pk: ['id']
    })
  })
  it('explains why other tabs are read-only', () => {
    expect(editability(undefined, rc('id'))).toMatchObject({ editable: false })
    expect(editability(table([col('id', true)], 'view'), rc('id'))).toMatchObject({
      editable: false,
      reason: expect.stringMatching(/view/)
    })
    expect(editability(table([col('v')]), rc('v'))).toMatchObject({
      editable: false,
      reason: expect.stringMatching(/primary key/)
    })
    expect(editability(table([col('id', true), col('v')]), rc('v'))).toMatchObject({
      editable: false
    })
  })
})

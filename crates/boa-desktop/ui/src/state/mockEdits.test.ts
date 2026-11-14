import { describe, expect, it } from 'vitest'
import { EMPTY_PENDING, addRow, buildStatements, deleteRow, setCell } from '@/lib/gridEdits'
import { sampleConnections } from './mock'
import { execute } from './mockData'
import { applyMockEdits, editedRows } from './mockEdits'

const db = sampleConnections()[0]!.databases[0]!
const users = db.schemas[0]!.tables.find((t) => t.name === 'users')!
const target = {
  schema: 'public',
  table: 'users',
  columns: users.columns.map((c) => c.name),
  pk: ['id']
}
const page = () => {
  const r = execute(db, 'select * from public.users', 5)
  if (r.kind !== 'rows') throw new Error(r.kind)
  return r.rows
}

describe('mock grid edits', () => {
  it('applies a valid batch and later pages show it', () => {
    const rows = page()
    let p = setCell(EMPTY_PENDING, rows, { kind: 'base', index: 0 }, 2, 'Renamed')
    p = deleteRow(p, { kind: 'base', index: 1 })
    const out = applyMockEdits(db, buildStatements(target, rows, p))
    expect(out.failed).toBeNull()
    expect(out.runs).toHaveLength(2)
    const after = page()
    expect(after[0]![2]).toBe('Renamed')
    expect(after[1]![0]).toBe(rows[2]![0])
    expect(editedRows(db.name, 'public', 'users')).not.toBeNull()
  })

  it('rolls back everything when a statement fails', () => {
    const rows = page()
    let p = setCell(EMPTY_PENDING, rows, { kind: 'base', index: 0 }, 2, 'Again')
    const n = addRow(p)
    // email is NOT NULL without a default.
    p = setCell(n.pending, rows, { kind: 'new', id: n.id }, 2, 'x')
    const out = applyMockEdits(db, buildStatements(target, rows, p))
    expect(out.failed).toMatchObject({ index: 1, error: { code: '23502' } })
    expect(page()[0]![2]).toBe(rows[0]![2])
  })

  it('fails an update whose row is gone', () => {
    const rows = page()
    const stale = [['999999999', ...rows[0]!.slice(1)]]
    const p = setCell(EMPTY_PENDING, stale, { kind: 'base', index: 0 }, 2, 'x')
    const out = applyMockEdits(db, buildStatements(target, stale, p))
    expect(out.failed).toMatchObject({ index: 0, error: { code: 'row_count' } })
  })
})

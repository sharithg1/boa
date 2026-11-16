import { describe, expect, it } from 'vitest'
import { filterAndSort, tailClauses } from './mockQuery'

const columns = [
  { name: 'id', type: 'bigint' },
  { name: 'role', type: 'text' },
  { name: 'Name', type: 'text' }
]
const rows = [
  ['1', 'admin', 'Ada'],
  ['2', 'member', null],
  ['10', 'member', 'Linus'],
  ['3', 'viewer', 'Grace']
]
const ids = (r: ReturnType<typeof filterAndSort>) =>
  r.kind === 'rows' ? r.rows.map((x) => x[0]) : [r.message]

describe('tailClauses', () => {
  it('finds WHERE and ORDER BY outside strings', () => {
    expect(tailClauses("\nwhere role = 'order by x' and id > 1\norder by id desc limit 5")).toEqual(
      {
        where: "role = 'order by x' and id > 1",
        orderBy: 'id desc'
      }
    )
    expect(tailClauses('')).toEqual({ where: undefined, orderBy: undefined })
  })
})

describe('filterAndSort', () => {
  it('filters with comparisons, like and null tests', () => {
    expect(ids(filterAndSort(columns, rows, { where: "role = 'member' and id > 2" }))).toEqual([
      '10'
    ])
    expect(ids(filterAndSort(columns, rows, { where: '"Name" ilike \'a%\' or id = 3' }))).toEqual([
      '1',
      '3'
    ])
    expect(ids(filterAndSort(columns, rows, { where: '"Name" is null' }))).toEqual(['2'])
  })

  it('sorts numerically, with nulls last ascending and first descending', () => {
    expect(ids(filterAndSort(columns, rows, { orderBy: 'id desc' }))).toEqual(['10', '3', '2', '1'])
    expect(ids(filterAndSort(columns, rows, { orderBy: 'role desc, id' }))).toEqual([
      '3',
      '2',
      '10',
      '1'
    ])
    expect(ids(filterAndSort(columns, rows, { orderBy: '"Name"' }))).toEqual(['1', '3', '10', '2'])
    expect(ids(filterAndSort(columns, rows, { orderBy: '"Name" desc' }))[0]).toBe('2')
  })

  it('fails like a server on unknown columns, and says what it cannot evaluate', () => {
    expect(filterAndSort(columns, rows, { where: 'nope = 1' })).toMatchObject({ code: '42703' })
    expect(filterAndSort(columns, rows, { orderBy: 'nope' })).toMatchObject({ code: '42703' })
    expect(filterAndSort(columns, rows, { where: 'id in (1, 2)' })).toMatchObject({
      code: '0A000'
    })
  })
})

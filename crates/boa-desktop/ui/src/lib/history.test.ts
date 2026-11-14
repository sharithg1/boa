import { describe, expect, it } from 'vitest'
import {
  duration,
  filterHistory,
  insertionText,
  localHistoryStore,
  mergeLoaded,
  mergeNewestFirst,
  outcomeLabel,
  sqlPreview,
  toRecord,
  type HistoryRecord
} from './history'

const rec = (id: string, sql: string, at: number, extra: Partial<HistoryRecord> = {}) => ({
  id,
  sql,
  database: 'app',
  at,
  ms: 5,
  ...extra
})

describe('toRecord', () => {
  it('keeps what matters and drops nulls and session-only fields', () => {
    expect(
      toRecord(
        {
          id: 7,
          sql: 'select 1',
          database: 'app',
          at: 10,
          ms: 2,
          rows: 1,
          offset: 0,
          source: 'console_1'
        },
        'r1'
      )
    ).toEqual({
      id: 'r1',
      sql: 'select 1',
      database: 'app',
      at: 10,
      ms: 2,
      rows: 1,
      source: 'console_1'
    })
    expect(
      toRecord(
        {
          id: 8,
          sql: 'select x',
          database: 'app',
          at: 11,
          ms: 3,
          rows: null,
          error: '[42703] no x',
          detail: 'Hint: y'
        },
        'r2'
      )
    ).toEqual({
      id: 'r2',
      sql: 'select x',
      database: 'app',
      at: 11,
      ms: 3,
      error: '[42703] no x',
      detail: 'Hint: y'
    })
    expect(
      toRecord(
        { id: 9, sql: 'delete from t', database: 'app', at: 1, ms: 1, rows: null, affected: 0 },
        'r3'
      ).affected
    ).toBe(0)
  })
  it('makes unique ids', () => {
    const e = { id: 1, sql: 's', database: 'd', at: 1, ms: 1, rows: null }
    expect(toRecord(e).id).not.toBe(toRecord(e).id)
  })
})

describe('filterHistory', () => {
  const items = [
    rec('1', 'SELECT * FROM users WHERE id = 1', 3),
    rec('2', 'select name from Orders', 2, { error: 'boom' }),
    rec('3', 'update users set name = 1', 1)
  ]
  it('returns everything for an empty query', () => {
    expect(filterHistory(items, { query: '  ' })).toBe(items)
  })
  it('matches every word, case-insensitively, in any order', () => {
    expect(filterHistory(items, { query: 'USERS' }).map((r) => r.id)).toEqual(['1', '3'])
    expect(filterHistory(items, { query: 'name users' }).map((r) => r.id)).toEqual(['3'])
    expect(filterHistory(items, { query: 'orders' }).map((r) => r.id)).toEqual(['2'])
    expect(filterHistory(items, { query: 'nothing' })).toEqual([])
  })
  it('filters errors only', () => {
    expect(filterHistory(items, { query: '', errorsOnly: true }).map((r) => r.id)).toEqual(['2'])
    expect(filterHistory(items, { query: 'users', errorsOnly: true })).toEqual([])
  })
  it('stays fast with thousands of entries', () => {
    const many = Array.from({ length: 5000 }, (_, i) =>
      rec(String(i), `select col_${i} from table_${i % 50} where x = ${i}`, i)
    )
    const t = performance.now()
    for (const q of ['s', 'se', 'sel', 'select', 'select table_7', 'select table_7 col_4'])
      filterHistory(many, { query: q })
    expect(filterHistory(many, { query: 'table_7 col_4007' }).map((r) => r.id)).toEqual(['4007'])
    expect(performance.now() - t).toBeLessThan(250)
  })
})

describe('merging', () => {
  it('merges connections newest first', () => {
    const a = [rec('a2', 'x', 20), rec('a1', 'x', 5)]
    const b = [rec('b1', 'x', 10)]
    expect(mergeNewestFirst([a, b]).map((r) => r.id)).toEqual(['a2', 'b1', 'a1'])
  })
  it('adds records made during a load without duplicates, capped', () => {
    const loaded = [rec('2', 'x', 2), rec('1', 'x', 1)]
    const pending = [rec('3', 'x', 3), rec('2', 'x', 2)]
    expect(mergeLoaded(loaded, pending).map((r) => r.id)).toEqual(['3', '2', '1'])
    expect(mergeLoaded(loaded, pending, 2).map((r) => r.id)).toEqual(['3', '2'])
  })
})

describe('display', () => {
  it('previews the first non-blank lines', () => {
    expect(sqlPreview('\n\n  select a,\n  b\nfrom t\n')).toEqual({
      text: '  select a,\n  b',
      more: true
    })
    expect(sqlPreview('select 1\r\n\r\n')).toEqual({ text: 'select 1', more: false })
    expect(sqlPreview('a\nb\n\n')).toEqual({ text: 'a\nb', more: false })
    expect(sqlPreview('   ')).toEqual({ text: '', more: false })
  })
  it('labels outcomes', () => {
    expect(outcomeLabel(rec('1', 'x', 1, { rows: 1 }))).toBe('1 row')
    expect(outcomeLabel(rec('1', 'x', 1, { rows: 500, truncated: true }))).toBe('500+ rows')
    expect(outcomeLabel(rec('1', 'x', 1, { rows: 0 }))).toBe('0 rows')
    expect(outcomeLabel(rec('1', 'x', 1, { affected: 3 }))).toBe('3 affected')
    expect(outcomeLabel(rec('1', 'x', 1, { affected: 0 }))).toBe('done')
    expect(outcomeLabel(rec('1', 'x', 1, { error: 'e', rows: 2 }))).toBe('error')
  })
  it('formats durations', () => {
    expect(duration(12)).toBe('12 ms')
    expect(duration(1234)).toBe('1.2 s')
    expect(duration(12_400)).toBe('12 s')
    expect(duration(125_000)).toBe('2m 5s')
  })
})

describe('localHistoryStore', () => {
  const memory = () => {
    const m = new Map<string, string>()
    return {
      getItem: (k: string) => m.get(k) ?? null,
      setItem: (k: string, v: string) => void m.set(k, v),
      removeItem: (k: string) => void m.delete(k)
    }
  }
  it('appends oldest first, caps, deletes and clears per connection', () => {
    const h = localHistoryStore(memory(), 3)
    for (const i of [1, 2, 3, 4]) h.append('c1', rec(String(i), 'x', i))
    h.append('c2', rec('z', 'x', 9))
    expect(h.list('c1').map((r) => r.id)).toEqual(['2', '3', '4'])
    h.remove('c1', ['3'])
    expect(h.list('c1').map((r) => r.id)).toEqual(['2', '4'])
    h.clear('c1')
    expect(h.list('c1')).toEqual([])
    expect(h.list('c2').map((r) => r.id)).toEqual(['z'])
  })
  it('survives missing or corrupt storage', () => {
    expect(localHistoryStore(undefined).list('c')).toEqual([])
    const kv = memory()
    kv.setItem('boa.history.c', '{nope')
    expect(localHistoryStore(kv).list('c')).toEqual([])
  })
})

describe('insertionText', () => {
  it('puts the statement on its own line at a line end or start', () => {
    expect(insertionText('select 1;', '', 'select 2')).toBe('\nselect 2')
    expect(insertionText('select 1;', '\nselect 3', 'select 2')).toBe('\nselect 2')
    expect(insertionText('', 'select 1;', 'select 2')).toBe('select 2\n')
    expect(insertionText('a\n', 'b', 'x')).toBe('x\n')
  })
  it('inserts as is on an empty line or mid-line', () => {
    expect(insertionText('', '', 'select 2')).toBe('select 2')
    expect(insertionText('a\n  ', '\nb', 'x')).toBe('x')
    expect(insertionText('select * from (', ') t', 'select 1')).toBe('select 1')
  })
})

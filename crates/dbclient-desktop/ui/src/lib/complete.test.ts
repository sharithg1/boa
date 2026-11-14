import { describe, expect, it } from 'vitest'
import type { Column, Schema, Table } from '@/state/types'
import { complete, matchTier } from './complete'

const col = (name: string, type = 'text', pk = false): Column => ({
  name,
  type,
  pk,
  nullable: true,
  default: null
})
const table = (name: string, columns: Column[], kind: Table['kind'] = 'table'): Table => ({
  name,
  kind,
  rows: 0,
  sizeBytes: 0,
  columns
})

const schemas: Schema[] = [
  {
    name: 'public',
    tables: [
      table('users', [col('id', 'integer', true), col('email'), col('created_at', 'timestamptz')]),
      table('orders', [
        col('id', 'integer', true),
        col('user_id', 'integer'),
        col('total', 'numeric')
      ]),
      table('active_users', [col('id', 'integer'), col('email')], 'view')
    ]
  },
  { name: 'billing', tables: [table('invoices', [col('id', 'integer', true), col('amount')])] }
]

/** Completes at the `|` in `sql`. */
function at(sql: string, explicit = true) {
  const pos = sql.indexOf('|')
  return complete(sql.replace('|', ''), pos, schemas, { defaultSchema: 'public', explicit })
}
const labels = (sql: string, explicit = true) => at(sql, explicit)?.items.map((i) => i.label) ?? []
const inserts = (sql: string) => at(sql)?.items.map((i) => i.insert) ?? []

describe('complete', () => {
  it('offers statement keywords at the start', () => {
    expect(labels('sel|')).toEqual(['select'])
    expect(labels('|')).toContain('insert into')
  })

  it('offers tables after FROM / JOIN, qualifying ones off the search path', () => {
    expect(labels('select * from u|')).toEqual(['users'])
    expect(labels('select * from users u join o|')).toEqual(['orders'])
    expect(inserts('select * from inv|')).toEqual(['billing.invoices'])
    expect(labels('select * from |')).toEqual(
      expect.arrayContaining(['users', 'orders', 'active_users', 'invoices', 'public', 'billing'])
    )
  })

  it('offers JOIN / WHERE after a table reference, not more tables', () => {
    expect(labels('select * from users u w|')).toEqual(['where'])
    expect(labels('select * from users l|')).toContain('left join')
  })

  it('offers columns of the tables in the statement', () => {
    const l = labels('select | from users')
    expect(l.slice(0, 3)).toEqual(['id', 'email', 'created_at'])
    expect(labels('select * from users where em|')).toEqual(['email'])
  })

  it('qualifies a column two tables share', () => {
    const items = at('select * from users u join orders o on |')!.items
    const ids = items.filter((i) => i.kind === 'column' && i.label.endsWith('.id'))
    expect(ids.map((i) => i.insert).sort()).toEqual(['o.id', 'u.id'])
    expect(items.find((i) => i.label === 'user_id')?.insert).toBe('user_id')
    expect(labels('select * from users u join orders o on u.id = o.user_id where i|')).toEqual(
      expect.arrayContaining(['u.id', 'o.id'])
    )
  })

  it('offers aliases in expressions', () => {
    expect(labels('select * from users usr where us|')).toContain('usr')
  })

  it('completes alias. / table. / schema.', () => {
    expect(labels('select u.| from users u', false)).toEqual(['id', 'email', 'created_at'])
    expect(labels('select users.e| from users')[0]).toBe('email')
    expect(labels('select * from billing.|', false)).toEqual(['invoices'])
    expect(labels('select billing.invoices.a| from billing.invoices')).toEqual(['amount'])
    expect(labels('select x.| from users u', false)).toEqual([])
  })

  it('offers keywords after an expression', () => {
    expect(labels('select * from users where id = 1 o|')).toEqual(
      expect.arrayContaining(['or', 'order by', 'offset'])
    )
    expect(labels('select * f|')[0]).toBe('from')
    expect(labels('select id f|')).toContain('from')
  })

  it('offers the target columns inside INSERT INTO t (…)', () => {
    expect(labels('insert into orders (|)')).toEqual(['id', 'user_id', 'total'])
    expect(labels('insert into orders (id, |)')).toContain('user_id')
  })

  it('offers columns in UPDATE … SET', () => {
    expect(labels('update users set e|')[0]).toBe('email')
  })

  it('reads tables from subqueries and CTEs', () => {
    expect(labels('with recent as (select * from orders) select * from r|')).toEqual(['recent'])
    expect(
      labels('select * from users where id in (select user_id from orders where to|)')[0]
    ).toBe('total')
  })

  it('offers types after :: and CAST(x AS', () => {
    expect(labels('select id::int|')).toContain('integer')
    expect(labels('select cast(id as tim|)')).toEqual(
      expect.arrayContaining(['timestamp', 'timestamptz'])
    )
  })

  it('stays quiet in strings, comments, numbers and alias names', () => {
    expect(at("select 'fr|")).toBeNull()
    expect(at('-- fr|')).toBeNull()
    expect(at('select 12|')).toBeNull()
    expect(at('select id as e|', false)).toBeNull()
  })

  it('only scopes the statement at the caret', () => {
    expect(labels('select * from orders; select | from users').includes('total')).toBe(false)
  })

  it('matches word starts and keeps keyword case', () => {
    expect(labels('select * from orders where uid|')).toContain('user_id')
    expect(inserts('SELECT * FROM users WH|')).toEqual(['WHERE'])
    expect(inserts('SELECT * FROM users |').every((s) => s === s.toUpperCase())).toBe(true)
  })

  it('needs a typed prefix unless invoked, and hides an already complete word', () => {
    expect(at('select * from |', false)).toBeNull()
    expect(at('select * from users wher|', false)?.items[0]?.label).toBe('where')
    expect(at('select * from users where|', false)).toBeNull()
  })

  it('replaces the whole word on demand', () => {
    const r = at('select * from us|ers')!
    expect([r.from, r.to, r.prefix]).toEqual([14, 19, 'us'])
  })

  it('completes inside an open quoted identifier', () => {
    const r = at('select * from "us|')!
    expect(r.from).toBe(14)
    expect(r.items[0]?.label).toBe('users')
  })
})

describe('matchTier', () => {
  it('ranks exact > prefix > word starts > substring', () => {
    expect(matchTier('users', 'users')).toBe(5)
    expect(matchTier('users', 'us')).toBe(4)
    expect(matchTier('created_at', 'crat')).toBe(3)
    expect(matchTier('created_at', 'ated')).toBe(2)
    expect(matchTier('created_at', 'zz')).toBe(0)
  })
})

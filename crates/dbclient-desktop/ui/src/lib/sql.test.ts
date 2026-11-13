import { describe, expect, it } from 'vitest'
import { tokenize, type Catalog } from './sql'

const catalog: Catalog = {
  schemas: new Set(['public']),
  tables: new Set(['users']),
  columns: new Set(['email', 'id'])
}

const kinds = (sql: string) =>
  tokenize(sql, catalog)
    .filter((t) => t.text.trim())
    .map((t) => `${t.kind}:${t.text.trim()}`)

describe('tokenize', () => {
  it('covers the input exactly', () => {
    const samples = [
      "select * from \"Clean\".\"T\" where a = 'it''s' -- tail\n/* open",
      "e'x' $$ body $$ $1 :name ::text 1.5e3 t1 [x]",
      ''
    ]
    for (const s of samples) expect(tokenize(s, catalog).map((t) => t.text).join('')).toBe(s)
  })

  it('classifies keywords, catalog names, literals and calls', () => {
    expect(kinds("SELECT id, count(*) FROM public.users WHERE email = 'a' AND id > 10")).toEqual([
      'keyword:SELECT',
      'column:id',
      'plain:,',
      'function:count',
      'plain:(',
      'operator:*',
      'plain:)',
      'keyword:FROM',
      'schema:public',
      'operator:.',
      'table:users',
      'keyword:WHERE',
      'column:email',
      'operator:=',
      "string:'a'",
      'keyword:AND',
      'column:id',
      'operator:>',
      'number:10'
    ])
  })

  it('treats words after a dot as names, not keywords', () => {
    expect(kinds('u.order')).toEqual(['plain:u', 'operator:.', 'plain:order'])
  })

  it('handles comments, casts, params and types', () => {
    expect(kinds("-- hi\nx::timestamptz /* c */ $1 :p")).toEqual([
      'comment:-- hi',
      'plain:x',
      'operator:::',
      'type:timestamptz',
      'comment:/* c */',
      'param:$1',
      'param::p'
    ])
  })
})

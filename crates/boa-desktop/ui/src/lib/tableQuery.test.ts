import { describe, expect, it } from 'vitest'
import {
  clauseError,
  cycleSort,
  identName,
  parseOrderBy,
  sortState,
  splitTopLevel,
  tableSelect
} from './tableQuery'

describe('tableSelect', () => {
  it('is the plain select without clauses', () => {
    expect(tableSelect('public', 'users')).toEqual({
      sql: 'select * from public.users',
      error: null
    })
    expect(tableSelect('public', 'users', { where: '  ', orderBy: '' }).sql).toBe(
      'select * from public.users'
    )
  })

  it('adds WHERE and ORDER BY, one per line, quoting the table', () => {
    expect(
      tableSelect('Sales', 'order', { where: "status = 'active' and id > 10", orderBy: 'id desc' })
    ).toEqual({
      sql: `select * from "Sales"."order"\nwhere status = 'active' and id > 10\norder by id desc`,
      error: null
    })
    expect(tableSelect('public', 'users', { orderBy: 'email' }).sql).toBe(
      'select * from public.users\norder by email'
    )
  })

  it('drops a typed-out WHERE / ORDER BY keyword', () => {
    expect(
      tableSelect('public', 'users', { where: 'WHERE id = 1', orderBy: 'order  by id' }).sql
    ).toBe('select * from public.users\nwhere id = 1\norder by id')
  })

  it('keeps a trailing line comment from swallowing the next clause', () => {
    const { sql, error } = tableSelect('public', 'users', { where: 'id = 1 -- me', orderBy: 'id' })
    expect(error).toBeNull()
    expect(sql.split('\n')).toEqual([
      'select * from public.users',
      'where id = 1 -- me',
      'order by id'
    ])
  })

  it('flags clauses that must not run, saying which one', () => {
    expect(tableSelect('public', 'users', { where: 'true; drop table users' }).error).toBe(
      'WHERE: Only one condition: remove the ";"'
    )
    expect(tableSelect('public', 'users', { orderBy: 'id; delete from users' }).error).toMatch(
      /^ORDER BY: /
    )
  })
})

describe('clauseError', () => {
  it('accepts semicolons inside strings, names, comments and dollar quotes', () => {
    for (const ok of [
      "name = 'a;b'",
      "name = 'it''s;'",
      "x = E'\\';'",
      '"we;ird" = 1',
      'id = 1 -- ; trailing',
      'id = 1 /* ; /* nested ; */ */',
      'body = $$;$$',
      'body = $t$ ; $t$',
      'id in (select id from t where a = (1))'
    ])
      expect(clauseError(ok), ok).toBeNull()
  })

  it('rejects a second statement and broken nesting', () => {
    expect(clauseError('true; drop table users')).toMatch(/;/)
    expect(clauseError('true;')).toMatch(/;/)
    expect(clauseError('1=1) union select 1 where (1=1')).toBe('Unbalanced parentheses')
    expect(clauseError('(a = 1')).toBe('Unbalanced parentheses')
    expect(clauseError("name = 'open")).toBe('Unterminated string literal')
    expect(clauseError('"open = 1')).toBe('Unterminated quoted identifier')
    expect(clauseError('a = 1 /* open')).toBe('Unterminated /* comment')
    expect(clauseError('a = $x$ open')).toBe('Unterminated $x$ string')
  })

  it('does not treat $1 or identifiers with $ as dollar quotes', () => {
    expect(clauseError('a$b = 1')).toBeNull()
  })
})

describe('splitTopLevel', () => {
  it('splits only outside parentheses and strings', () => {
    expect(splitTopLevel("a, coalesce(b, c), 'x,y' desc", ',')).toEqual([
      'a',
      'coalesce(b, c)',
      "'x,y' desc"
    ])
    expect(splitTopLevel("role = 'a and b' AND id > 1", '\\band\\b')).toEqual([
      "role = 'a and b'",
      'id > 1'
    ])
  })
})

describe('parseOrderBy', () => {
  it('reads direction, nulls and the column named', () => {
    expect(parseOrderBy('id, "Name" DESC nulls last, lower(email) asc')).toEqual([
      { text: 'id', expr: 'id', dir: 'asc', nulls: undefined, column: 'id' },
      {
        text: '"Name" DESC nulls last',
        expr: '"Name"',
        dir: 'desc',
        nulls: 'last',
        column: 'Name'
      },
      {
        text: 'lower(email) asc',
        expr: 'lower(email)',
        dir: 'asc',
        nulls: undefined,
        column: null
      }
    ])
    expect(parseOrderBy('  ')).toEqual([])
  })

  it('folds bare names to lower case like Postgres', () => {
    expect(identName('Email')).toBe('email')
    expect(identName('"Email"')).toBe('Email')
    expect(identName('"a""b"')).toBe('a"b')
    expect(identName('t.email')).toBeNull()
  })
})

describe('sortState', () => {
  it('maps sorted columns to direction and position', () => {
    expect([...sortState('created_at desc, lower(x), id')]).toEqual([
      ['created_at', { dir: 'desc', rank: 1 }],
      ['id', { dir: 'asc', rank: 3 }]
    ])
  })
})

describe('cycleSort', () => {
  it('goes ascending, descending, off', () => {
    const a = cycleSort('', 'email')
    expect(a).toBe('email')
    const b = cycleSort(a, 'email')
    expect(b).toBe('email desc')
    expect(cycleSort(b, 'email')).toBe('')
  })

  it('replaces the sort with another column, or appends with add', () => {
    expect(cycleSort('email desc', 'id')).toBe('id')
    expect(cycleSort('email desc', 'id', true)).toBe('email desc, id')
  })

  it('changes a column in place, keeping the other items as typed', () => {
    expect(cycleSort('lower(name), id NULLS FIRST, x', 'id')).toBe(
      'lower(name), id desc nulls first, x'
    )
    expect(cycleSort('lower(name), id desc, x', 'id')).toBe('lower(name), x')
  })

  it('quotes column names that need it', () => {
    expect(cycleSort('', 'Created At')).toBe('"Created At"')
    expect(cycleSort('"Created At"', 'Created At')).toBe('"Created At" desc')
    expect(cycleSort('', 'order')).toBe('"order"')
  })
})

import { describe, expect, it } from 'vitest'
import { destructiveRisks, needsConfirm, readOnlyEscape } from './guard'

const kinds = (sql: string) => destructiveRisks(sql).map((r) => r.kind)

describe('destructiveRisks', () => {
  it('flags UPDATE and DELETE without WHERE, naming the table', () => {
    expect(destructiveRisks('delete from public."Users"')).toEqual([
      {
        kind: 'no-where',
        message: 'DELETE without WHERE removes every row of public."Users"',
        statement: 'delete from public."Users"'
      }
    ])
    expect(destructiveRisks('UPDATE ONLY t SET a = 1')[0]?.message).toBe(
      'UPDATE without WHERE changes every row of t'
    )
    expect(kinds('update t set a = 1 from u')).toEqual(['no-where'])
  })

  it('accepts a WHERE at the statement level only', () => {
    expect(kinds('delete from t where id = 1')).toEqual([])
    expect(kinds('update t set a = (select b from u where u.id = 1)')).toEqual(['no-where'])
    expect(kinds('delete from t using u where t.id = u.id')).toEqual([])
    expect(kinds('update t set a = 1 where current of c')).toEqual([])
  })

  it('is not fooled by comments, strings and quoted names', () => {
    expect(kinds('delete from t -- where id = 1')).toEqual(['no-where'])
    expect(kinds('delete from t /* where id = 1 */')).toEqual(['no-where'])
    expect(kinds(`update t set note = 'where x'`)).toEqual(['no-where'])
    expect(kinds('update t set note = $$ where $$')).toEqual(['no-where'])
    expect(kinds('update t set "where" = 1')).toEqual(['no-where'])
    expect(kinds(`select 'delete from t'`)).toEqual([])
    expect(kinds('-- drop table t\nselect 1')).toEqual([])
    expect(kinds('select "drop", "truncate" from t')).toEqual([])
  })

  it('looks into CTEs, in both directions', () => {
    expect(kinds('with d as (delete from t returning *) select * from d where id > 1')).toEqual([
      'no-where'
    ])
    expect(kinds('with d as (delete from t where a returning *) select * from d')).toEqual([])
    expect(kinds('with x as (select 1 where true) update t set a = 1')).toEqual(['no-where'])
    expect(kinds('with x as (select 1) update t set a = 1 where id in (select * from x)')).toEqual(
      []
    )
  })

  it('ignores UPDATE / DELETE that are not commands', () => {
    expect(kinds('select * from t for update')).toEqual([])
    expect(
      kinds('create table c (p int references p (id) on delete cascade on update set null)')
    ).toEqual([])
    expect(kinds('grant select, update, delete on t to bob')).toEqual([])
    expect(kinds('insert into t values (1) on conflict (id) do update set a = 1')).toEqual([])
    expect(kinds('create trigger x before update or delete on t execute function f()')).toEqual([])
  })

  it('treats EXPLAIN as harmless unless it ANALYZEs', () => {
    expect(kinds('explain delete from t')).toEqual([])
    expect(kinds('explain (costs off) delete from t')).toEqual([])
    expect(kinds('explain analyze delete from t')).toEqual(['no-where'])
    expect(kinds('explain (analyze, buffers) update t set a = 1')).toEqual(['no-where'])
    expect(kinds('explain (analyze false) update t set a = 1')).toEqual([])
  })

  it('flags TRUNCATE and DROP', () => {
    expect(destructiveRisks('truncate table a, only b')[0]?.message).toBe(
      'TRUNCATE removes every row of a, b'
    )
    expect(destructiveRisks('DROP TABLE IF EXISTS s.a, b CASCADE')[0]?.message).toBe(
      'DROP TABLE s.a, b cannot be undone; CASCADE also drops everything that depends on it'
    )
    expect(destructiveRisks('drop materialized view v')[0]?.message).toBe(
      'DROP MATERIALIZED VIEW v cannot be undone'
    )
    expect(kinds('drop schema app')).toEqual(['drop'])
  })

  it('flags dropped columns, not dropped defaults or constraints', () => {
    expect(destructiveRisks('alter table t drop column c')[0]?.message).toBe(
      'DROP COLUMN c deletes column data from t'
    )
    expect(kinds('alter table t drop if exists c, add d int')).toEqual(['drop-column'])
    expect(kinds('alter table t drop c')).toEqual(['drop-column'])
    expect(kinds('alter table t alter column c drop default')).toEqual([])
    expect(kinds('alter table t alter column c drop not null')).toEqual([])
    expect(kinds('alter table t drop constraint t_pkey')).toEqual([])
  })

  it('checks every statement of a script, keeping each one', () => {
    const r = destructiveRisks(
      'select 1;\ndelete from a;\nupdate b set x = 1 where y;\ndrop view v;'
    )
    expect(r.map((x) => [x.kind, x.statement])).toEqual([
      ['no-where', 'delete from a'],
      ['drop', 'drop view v']
    ])
    expect(kinds('insert into t select * from u')).toEqual([])
    expect(kinds('')).toEqual([])
  })
})

describe('readOnlyEscape', () => {
  it('catches ways to make the session writable', () => {
    expect(readOnlyEscape('begin read write')).toMatch(/READ WRITE/)
    expect(readOnlyEscape('BEGIN ISOLATION LEVEL SERIALIZABLE, READ /* x */ WRITE')).toMatch(
      /READ WRITE/
    )
    expect(readOnlyEscape('set session characteristics as transaction read write')).toBeTruthy()
    expect(readOnlyEscape('SET default_transaction_read_only = off')).toBeTruthy()
    expect(readOnlyEscape('set transaction_read_only to false')).toBeTruthy()
    expect(
      readOnlyEscape(`select set_config('default_transaction_read_only', 'off', false)`)
    ).toBeTruthy()
  })
  it('lets reads of the setting and other statements through', () => {
    expect(readOnlyEscape('show default_transaction_read_only')).toBeNull()
    expect(readOnlyEscape(`select current_setting('transaction_read_only')`)).toBeNull()
    expect(readOnlyEscape(`select 'read write'`)).toBeNull()
    expect(readOnlyEscape('-- read write\nselect 1')).toBeNull()
    expect(readOnlyEscape('begin read only')).toBeNull()
  })
})

describe('needsConfirm', () => {
  const risks = destructiveRisks('truncate t')
  it('follows the preference, except on production', () => {
    expect(needsConfirm(risks, { confirmDestructive: true, production: false })).toBe(true)
    expect(needsConfirm(risks, { confirmDestructive: false, production: false })).toBe(false)
    expect(needsConfirm(risks, { confirmDestructive: false, production: true })).toBe(true)
    expect(needsConfirm([], { confirmDestructive: true, production: true })).toBe(false)
  })
})

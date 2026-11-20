import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { HistoryRecord } from '@/lib/history'

const disk = new Map<string, HistoryRecord[]>()
vi.mock('@/lib/backend', () => ({
  backend: {
    historyList: async (id: string, days: number) =>
      (disk.get(id) ?? []).filter((r) => !days || r.at >= Date.now() - days * 86_400_000),
    historyAppend: async (id: string, r: HistoryRecord) => {
      disk.set(id, [...(disk.get(id) ?? []), r])
    },
    historyDelete: async (id: string, ids: string[]) => {
      disk.set(
        id,
        (disk.get(id) ?? []).filter((r) => !ids.includes(r.id))
      )
    },
    historyClear: async (id: string) => {
      disk.delete(id)
    },
    runQuery: async (_id: string, _db: string, sql: string) => {
      if (sql.includes('bad')) throw { message: 'relation "bad" does not exist', code: '42P01' }
      return {
        columns: [{ name: 'a', type: 'int4' }],
        rows: [['1']],
        affected: 1,
        truncated: true,
        pageable: true,
        ms: 2
      }
    }
  },
  toBackendError: (e: unknown) => e
}))

const { clearHistory, deleteHistoryEntries, loadHistory, recordHistory, useQueryHistory } =
  await import('./history')
const { setPrefs } = await import('./prefs')
const { pageResult, runQuery, useStore } = await import('./store')

/** `at`: ms after the start of the test run (recent, so inside the retention period). */
const T0 = Date.now()
const DAY = 86_400_000
const entry = (sql: string, at: number) => ({
  id: at,
  sql,
  database: 'app',
  at: T0 + at,
  ms: 1,
  rows: 1
})
const ids = (id: string) => useQueryHistory.getState().lists[id]?.map((r) => r.sql)

describe('query history state', () => {
  beforeEach(() => {
    disk.clear()
    useQueryHistory.setState({ lists: {}, loading: {} })
    setPrefs({ recordHistory: true, historyDays: 30 })
  })

  it('saves runs, loads newest first and follows new runs', async () => {
    recordHistory('c1', entry('select a', 1))
    recordHistory('c1', entry('select b', 2))
    await loadHistory('c1')
    expect(ids('c1')).toEqual(['select b', 'select a'])
    recordHistory('c1', entry('select c', 3))
    expect(ids('c1')).toEqual(['select c', 'select b', 'select a'])
    expect(disk.get('c1')).toHaveLength(3)
  })

  it('keeps one entry per query, with the latest values', async () => {
    recordHistory('c1', entry('select * from users where id = 1', 1))
    recordHistory('c1', entry('select * from orders', 2))
    await loadHistory('c1')
    recordHistory('c1', entry('SELECT * FROM users\n  WHERE id = 2', 3))
    expect(ids('c1')).toEqual(['SELECT * FROM users\n  WHERE id = 2', 'select * from orders'])
    // A fresh read (the mocked disk keeps every append) dedupes the same way.
    await loadHistory('c1', { force: true })
    expect(ids('c1')).toEqual(['SELECT * FROM users\n  WHERE id = 2', 'select * from orders'])
    // The earlier run is removed from disk too.
    await Promise.resolve()
    expect(disk.get('c1')!.map((r) => r.at - T0)).toEqual([2, 3])
  })

  it('replaces an earlier run saved without a key, on disk too', async () => {
    disk.set('c1', [
      { id: 'old', sql: 'select * from users where id = 1', database: 'app', at: T0 - DAY, ms: 1 },
      { id: 'other', sql: 'select * from orders', database: 'app', at: T0 - DAY + 1, ms: 1 }
    ])
    await loadHistory('c1')
    recordHistory('c1', entry('select * from users where id = 2', 5))
    expect(ids('c1')).toEqual(['select * from users where id = 2', 'select * from orders'])
    await vi.waitFor(() =>
      expect(disk.get('c1')!.map((r) => r.sql)).toEqual([
        'select * from orders',
        'select * from users where id = 2'
      ])
    )
  })

  it('keeps the last 30 days by default, or the period set', async () => {
    recordHistory('c1', entry('select a', -40 * DAY))
    recordHistory('c1', entry('select b', -20 * DAY))
    recordHistory('c1', entry('select c', -2 * DAY))
    await loadHistory('c1')
    expect(ids('c1')).toEqual(['select c', 'select b'])
    setPrefs({ historyDays: 7 })
    await vi.waitFor(() => expect(ids('c1')).toEqual(['select c']))
    setPrefs({ historyDays: 0 })
    await vi.waitFor(() => expect(ids('c1')).toEqual(['select c', 'select b', 'select a']))
  })

  it('keeps runs made while loading', async () => {
    recordHistory('c1', entry('old', 1))
    const p = loadHistory('c1')
    recordHistory('c1', entry('during', 2))
    await p
    expect(ids('c1')).toEqual(['during', 'old'])
  })

  it('records nothing when turned off', async () => {
    setPrefs({ recordHistory: false })
    recordHistory('c1', entry('select 1', 1))
    await loadHistory('c1')
    expect(ids('c1')).toEqual([])
    expect(disk.has('c1')).toBe(false)
  })

  it('deletes entries and clears a connection', async () => {
    recordHistory('c1', entry('a', 1))
    recordHistory('c1', entry('b', 2))
    recordHistory('c2', entry('c', 3))
    await loadHistory('c1')
    const first = useQueryHistory.getState().lists.c1![1]!
    await deleteHistoryEntries('c1', [first.id])
    expect(ids('c1')).toEqual(['b'])
    expect(disk.get('c1')!.map((r) => r.sql)).toEqual(['b'])
    await clearHistory('c1')
    expect(ids('c1')).toEqual([])
    expect(disk.has('c1')).toBe(false)
    expect(disk.get('c2')).toHaveLength(1)
  })

  it('records console runs and failures, not page re-fetches', async () => {
    useStore.setState({
      connections: { c1: { id: 'c1', name: 'local', state: 'connected', databases: [] } as never }
    })
    await runQuery('c1', 'app', 't1', 'console_1', 'select * from big')
    await pageResult('t1', 500)
    await pageResult('t1', 1000)
    await runQuery('c1', 'app', 't1', 'console_1', 'select * from bad')
    const saved = disk.get('c1')!
    expect(saved.map((r) => r.sql)).toEqual(['select * from big', 'select * from bad'])
    expect(saved[0]).toMatchObject({ rows: 1, truncated: true, source: 'console_1' })
    expect(saved[1]!.error).toBe('[42P01] relation "bad" does not exist')
    // The Output log still shows every fetch.
    expect(useStore.getState().history.c1).toHaveLength(4)
  })
})

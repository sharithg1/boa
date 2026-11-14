import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { HistoryRecord } from '@/lib/history'

const disk = new Map<string, HistoryRecord[]>()
vi.mock('@/lib/backend', () => ({
  backend: {
    historyList: async (id: string) => [...(disk.get(id) ?? [])],
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

const entry = (sql: string, at: number) => ({ id: at, sql, database: 'app', at, ms: 1, rows: 1 })
const ids = (id: string) => useQueryHistory.getState().lists[id]?.map((r) => r.sql)

describe('query history state', () => {
  beforeEach(() => {
    disk.clear()
    useQueryHistory.setState({ lists: {}, loading: {} })
    setPrefs({ recordHistory: true })
  })

  it('saves runs, loads newest first and follows new runs', async () => {
    recordHistory('c1', entry('select 1', 1))
    recordHistory('c1', entry('select 2', 2))
    await loadHistory('c1')
    expect(ids('c1')).toEqual(['select 2', 'select 1'])
    recordHistory('c1', entry('select 3', 3))
    expect(ids('c1')).toEqual(['select 3', 'select 2', 'select 1'])
    expect(disk.get('c1')).toHaveLength(3)
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

import { describe, expect, it } from 'vitest'
import { nextTabNumber, parseSession } from './session'
import { dbKey, tableKey } from './store'

const saved = {
  active: { connectionId: 'c1', database: 'app' },
  workspaces: {
    [dbKey('c1', 'app')]: {
      tabs: [
        { id: 'q3', kind: 'query', title: 'console_1', sql: 'select 1' },
        { id: 't12', kind: 'table', schema: 'public', table: 'users', preview: true }
      ],
      activeTab: 't12'
    },
    [dbKey('gone', 'app')]: {
      tabs: [{ id: 'q40', kind: 'query', title: 'console_1', sql: '' }],
      activeTab: 'q40'
    }
  },
  mru: [tableKey('c1', 'app', 'public', 'users'), tableKey('gone', 'app', 'public', 'x')],
  seen: {
    [tableKey('c1', 'app', 'public', 'users')]: 5,
    [tableKey('gone', 'app', 'public', 'x')]: 6
  },
  sidebarOpen: false,
  bottomOpen: true
}

describe('parseSession', () => {
  it('restores tabs, their SQL and the active tab', () => {
    const s = parseSession(JSON.stringify(saved), ['c1'])!
    expect(s.active).toEqual(saved.active)
    expect(s.workspaces[dbKey('c1', 'app')]).toEqual(saved.workspaces[dbKey('c1', 'app')])
    expect(s.sidebarOpen).toBe(false)
    expect(s.rightPanelOpen).toBeUndefined()
  })

  it('drops everything of connections that no longer exist', () => {
    const s = parseSession(JSON.stringify(saved), ['c1'])!
    expect(Object.keys(s.workspaces)).toEqual([dbKey('c1', 'app')])
    expect(s.mru).toEqual([tableKey('c1', 'app', 'public', 'users')])
    expect(Object.keys(s.seen)).toEqual([tableKey('c1', 'app', 'public', 'users')])
    expect(parseSession(JSON.stringify(saved), ['other'])!.active).toBeNull()
  })

  it('skips malformed and duplicate tabs and repairs the active tab', () => {
    const raw = JSON.stringify({
      workspaces: {
        [dbKey('c1', 'app')]: {
          tabs: [
            { id: 'q1', kind: 'query', title: 'console_1' },
            { id: 'q2', kind: 'query', title: 'console_2', sql: 'a' },
            { id: 'q2', kind: 'query', title: 'console_3', sql: 'b' },
            'junk'
          ],
          activeTab: 'q1'
        },
        [dbKey('c1', 'empty')]: { tabs: [], activeTab: null }
      }
    })
    const s = parseSession(raw, ['c1'])!
    expect(s.workspaces).toEqual({
      [dbKey('c1', 'app')]: {
        tabs: [{ id: 'q2', kind: 'query', title: 'console_2', sql: 'a' }],
        activeTab: 'q2'
      }
    })
  })

  it('ignores missing or corrupt data', () => {
    expect(parseSession(null, ['c1'])).toBeNull()
    expect(parseSession('{nope', ['c1'])).toBeNull()
    expect(parseSession('[]', ['c1'])).toBeNull()
  })
})

describe('nextTabNumber', () => {
  it('is past every restored tab id', () => {
    expect(nextTabNumber(parseSession(JSON.stringify(saved), ['c1'])!)).toBe(13)
    expect(nextTabNumber(parseSession('{}', [])!)).toBe(1)
  })
})

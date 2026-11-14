import { beforeEach, describe, expect, it } from 'vitest'
import type { Backend } from '@/lib/backend'
import { forgetTx, noteTx, openTransactions, trackTx, txOf, useTx } from './tx'

const ok = { columns: [], rows: [], affected: 0, truncated: false, pageable: false, ms: 1 }

function fake(over: Partial<Backend>): Backend {
  return over as Backend
}

describe('transaction state', () => {
  beforeEach(() => useTx.setState({ sessions: {} }))

  it('is kept per session and listed while open', () => {
    expect(txOf('c', 'db')).toEqual({ mode: 'auto', state: 'idle' })
    noteTx('c', 'db', { state: 'active' })
    noteTx('c', 'other', { mode: 'manual' })
    noteTx('d', 'db', { state: 'failed' })
    const s = useTx.getState().sessions
    expect(openTransactions(s)).toEqual([
      { connectionId: 'c', database: 'db', state: 'active' },
      { connectionId: 'd', database: 'db', state: 'failed' }
    ])
    expect(openTransactions(s, 'c')).toHaveLength(1)
    forgetTx('c')
    expect(txOf('c', 'db').state).toBe('idle')
    expect(txOf('c', 'other').mode).toBe('auto')
    expect(txOf('d', 'db').state).toBe('failed')
  })

  it('follows what the backend reports, on success and on error', async () => {
    let next: 'ok' | 'err' = 'ok'
    const b = trackTx(
      fake({
        runQuery: async () => {
          if (next === 'err') throw { message: 'aborted', code: '25P02', tx: 'failed' }
          return { ...ok, tx: 'active' }
        },
        setTxMode: async () => 'active',
        disconnect: async () => {}
      })
    )
    await b.runQuery('c', 'db', 'begin', 10)
    expect(txOf('c', 'db').state).toBe('active')
    next = 'err'
    await expect(b.runQuery('c', 'db', 'select 1', 10)).rejects.toMatchObject({ code: '25P02' })
    expect(txOf('c', 'db').state).toBe('failed')
    await b.setTxMode('c', 'db', true)
    expect(txOf('c', 'db')).toEqual({ mode: 'manual', state: 'active' })
    await b.disconnect('c')
    expect(txOf('c', 'db')).toEqual({ mode: 'auto', state: 'idle' })
  })

  it('follows submitted grid edits (Manual mode leaves a transaction open)', async () => {
    const b = trackTx(
      fake({
        applyEdits: async () => ({
          runs: [],
          failed: null,
          inTransaction: true,
          tx: 'active',
          ms: 1
        })
      })
    )
    await b.applyEdits('c', 'db', [])
    expect(txOf('c', 'db').state).toBe('active')
  })
})

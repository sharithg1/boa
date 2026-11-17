import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { backend } from '@/lib/backend'
import {
  checkConnections,
  disconnect,
  ensureConnected,
  RETRY_DELAYS_MS,
  recover,
  runQuery,
  useStore
} from './store'
import { noteTx, txOf, useTx } from './tx'
import type { Connection } from './types'

const conn = (state: Connection['state']): Connection => ({
  id: 'c1',
  name: 'Local',
  engine: 'postgres',
  host: 'localhost',
  port: 5432,
  user: 'me',
  database: 'app',
  sslMode: 'prefer',
  hasPassword: true,
  readOnly: false,
  state,
  latencyMs: null,
  serverVersion: null,
  databases: [{ name: 'app', schemas: [], loaded: true, schemaNames: [] }]
})

const unreachable = { message: 'Connection refused', code: 'unreachable' }
const state = () => useStore.getState().connections.c1?.state
const toasts = () => useStore.getState().toasts.map((t) => t.title)

/** Lets pending promise callbacks run (fake timers don't advance them). */
const settle = () => vi.advanceTimersByTimeAsync(0)

describe('reconnecting', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    useStore.setState({
      connections: { c1: conn('connected') },
      connectionOrder: ['c1'],
      wanted: { c1: true },
      toasts: [],
      passwordPrompt: null
    })
    useTx.setState({ sessions: {} })
  })
  afterEach(async () => {
    // End any recovery left running so tests don't leak into each other.
    vi.spyOn(backend, 'disconnect').mockResolvedValue()
    await disconnect('c1')
    vi.restoreAllMocks()
    vi.useRealTimers()
  })

  it('keeps a connection that still answers, without showing anything', async () => {
    const ping = vi.spyOn(backend, 'ping').mockResolvedValue({ lostTx: [] })
    await recover('c1', { reason: 'wake' })
    expect(ping).toHaveBeenCalledTimes(1)
    expect(state()).toBe('connected')
    expect(toasts()).toEqual([])
  })

  it('shows reconnecting while the server is away, and says when it is back', async () => {
    const ping = vi
      .spyOn(backend, 'ping')
      .mockRejectedValueOnce(unreachable)
      .mockRejectedValueOnce(unreachable)
      .mockResolvedValue({ lostTx: [] })
    const done = recover('c1')
    await settle()
    expect(state()).toBe('reconnecting')
    await vi.advanceTimersByTimeAsync(RETRY_DELAYS_MS[0]!)
    expect(state()).toBe('reconnecting')
    await vi.advanceTimersByTimeAsync(RETRY_DELAYS_MS[1]!)
    expect(await done).toBe(true)
    expect(ping).toHaveBeenCalledTimes(3)
    expect(state()).toBe('connected')
    expect(toasts()).toEqual(['Reconnected to Local'])
  })

  it('gives up after the last retry, offering to retry', async () => {
    vi.spyOn(backend, 'ping').mockRejectedValue(unreachable)
    const done = recover('c1')
    for (const ms of RETRY_DELAYS_MS) await vi.advanceTimersByTimeAsync(ms)
    expect(await done).toBe(false)
    expect(state()).toBe('failed')
    const toast = useStore.getState().toasts[0]!
    expect(toast.title).toBe('Lost connection to Local')
    expect(toast.action?.label).toBe('Retry')
  })

  it('connects again when the backend no longer has the connection open', async () => {
    vi.spyOn(backend, 'ping').mockRejectedValue({ message: 'Not connected', code: 'not_connected' })
    const connect = vi
      .spyOn(backend, 'connect')
      .mockResolvedValue({ serverVersion: 'PostgreSQL 17', databases: ['app'] })
    vi.spyOn(backend, 'listSchemas').mockResolvedValue([])
    useStore.setState({ connections: { c1: conn('disconnected') } })
    expect(await recover('c1', { reason: 'restore' })).toBe(true)
    expect(connect).toHaveBeenCalledTimes(1)
    expect(state()).toBe('connected')
    expect(toasts()).toEqual([])
  })

  it('stops on a refused login, skipping the password prompt in the background', async () => {
    vi.spyOn(backend, 'ping').mockRejectedValue({ message: 'Not connected', code: 'not_connected' })
    vi.spyOn(backend, 'connect').mockRejectedValue({
      message: 'no password',
      code: 'password_required'
    })
    useStore.setState({ connections: { c1: { ...conn('disconnected'), hasPassword: false } } })
    expect(await recover('c1', { reason: 'restore', quiet: true })).toBe(false)
    expect(useStore.getState().passwordPrompt).toBeNull()
    expect(useStore.getState().wanted).toEqual({})
  })

  it('warns about transactions rolled back with a dropped session', async () => {
    noteTx('c1', 'app', { state: 'active' })
    vi.spyOn(backend, 'ping').mockResolvedValue({ lostTx: ['app'] })
    await recover('c1', { reason: 'wake' })
    expect(txOf('c1', 'app').state).toBe('idle')
    expect(toasts()).toEqual(['Transaction rolled back'])
  })

  it('starts reconnecting when a call fails on the network', async () => {
    vi.spyOn(backend, 'ping').mockRejectedValue(unreachable)
    // The real backend's `runQuery` goes through `trackLinks`: fail beneath it.
    useStore.setState({ connections: { c1: conn('connected') } })
    const { trackLinks } = await import('./links')
    const tracked = trackLinks({
      ...backend,
      runQuery: () => Promise.reject({ message: 'gone', code: 'connection_lost' })
    })
    await expect(tracked.runQuery('c1', 'app', 'select 1', 10)).rejects.toMatchObject({
      code: 'connection_lost'
    })
    await settle()
    expect(state()).toBe('reconnecting')
  })

  it('a wake-up retries a failed wanted connection right away', async () => {
    useStore.setState({ connections: { c1: conn('failed') } })
    const ping = vi.spyOn(backend, 'ping').mockResolvedValue({ lostTx: [] })
    checkConnections()
    await settle()
    expect(ping).toHaveBeenCalledTimes(1)
    expect(state()).toBe('connected')
  })

  it('disconnecting stops reconnecting and forgets the connection was wanted', async () => {
    const ping = vi.spyOn(backend, 'ping').mockRejectedValue(unreachable)
    vi.spyOn(backend, 'disconnect').mockResolvedValue()
    const done = recover('c1')
    await settle()
    await disconnect('c1')
    expect(await done).toBe(false)
    await vi.advanceTimersByTimeAsync(60_000)
    expect(ping).toHaveBeenCalledTimes(1)
    expect(state()).toBe('disconnected')
    expect(useStore.getState().wanted).toEqual({})
  })

  it('running a query connects first', async () => {
    useStore.setState({ connections: { c1: conn('disconnected') } })
    vi.spyOn(backend, 'connect').mockResolvedValue({ serverVersion: 'PG', databases: ['app'] })
    vi.spyOn(backend, 'listSchemas').mockResolvedValue([])
    const run = vi.spyOn(backend, 'runQuery').mockResolvedValue({
      columns: [],
      rows: [],
      affected: 1,
      truncated: false,
      pageable: false,
      ms: 1
    })
    expect(await ensureConnected('c1')).toBe(true)
    useStore.setState({ connections: { c1: conn('disconnected') } })
    await runQuery('c1', 'app', 'q1', 'console_1', 'insert into t values (1)')
    expect(state()).toBe('connected')
    expect(run).toHaveBeenCalledTimes(1)
  })
})

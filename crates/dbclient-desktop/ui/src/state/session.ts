// The last session, restored at startup: the selected database, each
// database's tabs (consoles with their SQL, open tables) and its active
// tab, recent tables and which panels were open. Kept in localStorage and
// rewritten as it changes, so a crash or force quit loses nothing either.
// Results are not kept: statements are not re-run on launch.

import {
  type Active,
  connect,
  loadDatabase,
  reserveIds,
  type Tab,
  useStore,
  type Workspace
} from './store'

export type Session = {
  active: Active | null
  workspaces: Record<string, Workspace>
  mru: string[]
  seen: Record<string, number>
  sidebarOpen?: boolean
  rightPanelOpen?: boolean
  bottomOpen?: boolean
}

type SessionState = Required<Session>

const KEY = 'dbclient.session.v1'
const SAVE_DELAY_MS = 250

const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === 'object' && v !== null && !Array.isArray(v)

/** Connection id of a `dbKey` / `tableKey`. */
const connectionOf = (key: string) => key.split('\u0000')[0]!

function parseTab(v: unknown): Tab | null {
  if (!isObject(v) || typeof v.id !== 'string') return null
  if (v.kind === 'query' && typeof v.title === 'string' && typeof v.sql === 'string')
    return { id: v.id, kind: 'query', title: v.title, sql: v.sql }
  if (v.kind === 'table' && typeof v.schema === 'string' && typeof v.table === 'string')
    return {
      id: v.id,
      kind: 'table',
      schema: v.schema,
      table: v.table,
      preview: v.preview === true
    }
  return null
}

/**
 * Reads a saved session, keeping only what still makes sense: well-formed
 * tabs with unique ids, and nothing of connections that no longer exist.
 */
export function parseSession(raw: string | null, connectionIds: Iterable<string>): Session | null {
  if (!raw) return null
  let data: unknown
  try {
    data = JSON.parse(raw)
  } catch {
    return null
  }
  if (!isObject(data)) return null
  const known = new Set(connectionIds)
  const ids = new Set<string>()

  const workspaces: Record<string, Workspace> = {}
  for (const [key, ws] of Object.entries(isObject(data.workspaces) ? data.workspaces : {})) {
    if (!known.has(connectionOf(key)) || !isObject(ws) || !Array.isArray(ws.tabs)) continue
    const tabs: Tab[] = []
    for (const t of ws.tabs) {
      const tab = parseTab(t)
      if (!tab || ids.has(tab.id)) continue
      ids.add(tab.id)
      tabs.push(tab)
    }
    if (!tabs.length) continue
    const activeTab = tabs.some((t) => t.id === ws.activeTab)
      ? (ws.activeTab as string)
      : tabs[0]!.id
    workspaces[key] = { tabs, activeTab }
  }

  const a = data.active
  const active =
    isObject(a) &&
    typeof a.connectionId === 'string' &&
    typeof a.database === 'string' &&
    known.has(a.connectionId)
      ? { connectionId: a.connectionId, database: a.database }
      : null

  const mru = (Array.isArray(data.mru) ? data.mru : []).filter(
    (k): k is string => typeof k === 'string' && known.has(connectionOf(k))
  )
  const seen = Object.fromEntries(
    Object.entries(isObject(data.seen) ? data.seen : {}).filter(
      (e): e is [string, number] => typeof e[1] === 'number' && known.has(connectionOf(e[0]))
    )
  )
  const flag = (v: unknown) => (typeof v === 'boolean' ? v : undefined)
  return {
    active,
    workspaces,
    mru,
    seen,
    sidebarOpen: flag(data.sidebarOpen),
    rightPanelOpen: flag(data.rightPanelOpen),
    bottomOpen: flag(data.bottomOpen)
  }
}

/** One past the highest number used in the session's tab ids (`q12` → 13). */
export function nextTabNumber(session: Session): number {
  let max = 0
  for (const ws of Object.values(session.workspaces))
    for (const t of ws.tabs) max = Math.max(max, Number(/(\d+)$/.exec(t.id)?.[1] ?? 0))
  return max + 1
}

const pick = (s: SessionState): SessionState => ({
  active: s.active,
  workspaces: s.workspaces,
  mru: s.mru,
  seen: s.seen,
  sidebarOpen: s.sidebarOpen,
  rightPanelOpen: s.rightPanelOpen,
  bottomOpen: s.bottomOpen
})

function save() {
  try {
    localStorage.setItem(KEY, JSON.stringify(pick(useStore.getState())))
  } catch {
    // Storage unavailable or full: the session is just not kept.
  }
}

let started = false

/**
 * Restores the last session (call once the connections are loaded),
 * reconnects the connection it was on, and from then on saves changes.
 */
export function startSession() {
  if (started || typeof localStorage === 'undefined') return
  started = true

  let raw: string | null = null
  try {
    raw = localStorage.getItem(KEY)
  } catch {
    // Storage unavailable: start empty.
  }
  const session = parseSession(raw, Object.keys(useStore.getState().connections))
  if (session) {
    reserveIds(nextTabNumber(session))
    const { sidebarOpen, rightPanelOpen, bottomOpen, ...rest } = session
    useStore.setState((s) => ({
      ...rest,
      sidebarOpen: sidebarOpen ?? s.sidebarOpen,
      rightPanelOpen: rightPanelOpen ?? s.rightPanelOpen,
      bottomOpen: bottomOpen ?? s.bottomOpen
    }))
    const { active } = session
    if (active)
      void connect(active.connectionId).then((ok) => {
        if (ok) void loadDatabase(active.connectionId, active.database)
      })
  }

  let timer: ReturnType<typeof setTimeout> | undefined
  const flush = () => {
    if (timer === undefined) return
    clearTimeout(timer)
    timer = undefined
    save()
  }
  useStore.subscribe((s, prev) => {
    const a = pick(s)
    const b = pick(prev)
    if ((Object.keys(a) as (keyof SessionState)[]).every((k) => a[k] === b[k])) return
    clearTimeout(timer)
    timer = setTimeout(flush, SAVE_DELAY_MS)
  })
  // Quit or reload within the delay: write what is pending.
  window.addEventListener('pagehide', flush)
  window.addEventListener('beforeunload', flush)
}

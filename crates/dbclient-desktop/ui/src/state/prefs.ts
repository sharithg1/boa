// Client-side preferences (Settings → General), persisted in localStorage.

import { create } from 'zustand'

export type RightTab = 'structure' | 'info' | 'history'

type PrefsListener = (next: Prefs, prev: Prefs) => void
const listeners = new Set<PrefsListener>()

/** Called after every `setPrefs` / `resetPrefs`. */
export function onPrefsChange(cb: PrefsListener): () => void {
  listeners.add(cb)
  return () => listeners.delete(cb)
}

export type Prefs = {
  /** Rows fetched when a table tab opens. */
  rowLimit: number
  /** Ask before running UPDATE / DELETE / DROP without a WHERE. */
  confirmDestructive: boolean
  /** Approximate row counts next to tables in the sidebar. */
  sidebarRowCounts: boolean
  /** Show views alongside tables in the sidebar. */
  sidebarViews: boolean
  /** Number of "Recent" entries in the ⌘K palette. */
  recentCount: number
  /** Sidebar layout (drag to reorder): connection ids; those not listed follow. */
  connectionOrder: string[]
  /** Expanded Database Explorer nodes, by tree path (see Sidebar `nodeId`). */
  treeExpanded: Record<string, true>
  /**
   * What the explorer shows and introspects, DataGrip's "N of M": by
   * connection id the databases, by `connectionId/database` the schemas.
   * `'*'` is all (including ones created later); unset is the default (the
   * connection's database; the `public` schema).
   */
  scopes: Record<string, string[] | '*'>
  /** Bottom panel (Database Output) height in px, as last dragged. */
  bottomHeight: number
  /** Sidebar width in px, as last dragged. */
  sidebarWidth: number
  /** Right panel width in px, as last dragged. */
  rightPanelWidth: number
  /** Right panel tab last selected. */
  rightTab: RightTab
}

/** Fields kept by "Reset to defaults": layout, not settings. */
const KEPT_ON_RESET = [
  'connectionOrder',
  'treeExpanded',
  'scopes',
  'bottomHeight',
  'sidebarWidth',
  'rightPanelWidth',
  'rightTab'
] as const

export const DEFAULT_PREFS: Prefs = {
  rowLimit: 500,
  confirmDestructive: true,
  sidebarRowCounts: true,
  sidebarViews: true,
  recentCount: 5,
  connectionOrder: [],
  treeExpanded: {},
  scopes: {},
  bottomHeight: 280,
  sidebarWidth: 260,
  rightPanelWidth: 340,
  rightTab: 'structure'
}

const KEY = 'dbclient.prefs.v2'

const RIGHT_TABS: RightTab[] = ['structure', 'info', 'history']

function load(): Prefs {
  try {
    const raw = typeof localStorage !== 'undefined' ? localStorage.getItem(KEY) : null
    if (!raw) return DEFAULT_PREFS
    const prefs = { ...DEFAULT_PREFS, ...(JSON.parse(raw) as Partial<Prefs>) }
    if (!RIGHT_TABS.includes(prefs.rightTab)) prefs.rightTab = DEFAULT_PREFS.rightTab
    return prefs
  } catch {
    return DEFAULT_PREFS
  }
}

export const usePrefs = create<Prefs>(load)

export function setPrefs(patch: Partial<Prefs>): void {
  const prev = usePrefs.getState()
  usePrefs.setState(patch)
  const next = usePrefs.getState()
  try {
    localStorage.setItem(KEY, JSON.stringify(next))
  } catch {
    // Storage unavailable (private mode): keep the in-memory value.
  }
  for (const cb of listeners) cb(next, prev)
}

export function resetPrefs(): void {
  const cur = usePrefs.getState()
  const kept = Object.fromEntries(KEPT_ON_RESET.map((k) => [k, cur[k]])) as Partial<Prefs>
  setPrefs({ ...DEFAULT_PREFS, ...kept })
}

/** Sidebar width limits for the drag handle. */
export const SIDEBAR_MIN_WIDTH = 200
export const SIDEBAR_MAX_WIDTH = 480

export function clampSidebarWidth(px: number): number {
  if (!Number.isFinite(px)) return DEFAULT_PREFS.sidebarWidth
  return Math.round(Math.min(SIDEBAR_MAX_WIDTH, Math.max(SIDEBAR_MIN_WIDTH, px)))
}

/** Right panel width limits for the drag handle. */
export const RIGHT_PANEL_MIN_WIDTH = 280
export const RIGHT_PANEL_MAX_WIDTH = 800

export function clampRightPanelWidth(px: number): number {
  if (!Number.isFinite(px)) return DEFAULT_PREFS.rightPanelWidth
  return Math.round(Math.min(RIGHT_PANEL_MAX_WIDTH, Math.max(RIGHT_PANEL_MIN_WIDTH, px)))
}

/** `ids` in the saved `order` first, then the rest in their original order. */
export function applyOrder(ids: readonly string[], order: readonly string[] | undefined): string[] {
  if (!order?.length) return [...ids]
  const rank = new Map(order.map((id, i) => [id, i]))
  const known = ids.filter((id) => rank.has(id)).sort((a, b) => rank.get(a)! - rank.get(b)!)
  return [...known, ...ids.filter((id) => !rank.has(id))]
}

/** Bottom panel height limits for the drag handle. */
export const BOTTOM_MIN_HEIGHT = 120
export const BOTTOM_MAX_HEIGHT = 900

export function clampBottomHeight(px: number): number {
  if (!Number.isFinite(px)) return DEFAULT_PREFS.bottomHeight
  const max = typeof window === 'undefined' ? BOTTOM_MAX_HEIGHT : window.innerHeight - 200
  return Math.round(Math.min(Math.min(BOTTOM_MAX_HEIGHT, max), Math.max(BOTTOM_MIN_HEIGHT, px)))
}

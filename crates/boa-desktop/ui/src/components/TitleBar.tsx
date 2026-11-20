// Header of the main column (Cursor-style: the sidebar and the right panel
// each have their own header row, so this one only spans the work area). It
// holds a compact database selector (opens the ⌘K palette), the database's
// tabs as pills (queries and open tables), a "+" menu, a "…" menu of
// connection actions and, when the right panel is closed, its toggle. With
// the sidebar closed it also takes the traffic-light inset and the sidebar
// toggle. Tabs activate on click, close on × or middle click, pin on double
// click and reorder by dragging.
// On macOS the window uses an overlay title bar, so every non-interactive
// part of this row drags the window (double click zooms); see
// lib/window-chrome.ts.

import { useRef, useState, type PointerEvent as ReactPointerEvent } from 'react'
import {
  ChevronDown,
  Copy,
  Database,
  Eye,
  FileCode,
  Info,
  MoreHorizontal,
  PanelRight,
  Plus,
  Search,
  Table2,
  Unplug,
  X
} from 'lucide-react'
import { cn } from '@/lib/utils'
import { useTitleBarInset } from '@/lib/window-chrome'
import {
  activateTab,
  closeTab,
  connectionString,
  dbKey,
  disconnect,
  findTable,
  moveTab,
  newQueryTab,
  pinTab,
  pushToast,
  tabLabel,
  useStore
} from '@/state/store'
import { setPrefs } from '@/state/prefs'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger
} from '@/components/ui/dropdown-menu'
import { IconButton } from './IconButton'
import { Spinner } from '@/components/ui/spinner'
import { SidebarToggle } from './Sidebar'
import { connectionColor } from '@/lib/connTag'

export function TitleBar() {
  const sidebarOpen = useStore((s) => s.sidebarOpen)
  const rightPanelOpen = useStore((s) => s.rightPanelOpen)
  const active = useStore((s) => s.active)
  const inset = useTitleBarInset()
  return (
    <header
      // "deep": any non-interactive descendant (empty tab strip, spacer)
      // drags the window; buttons and tabs keep their clicks.
      data-tauri-drag-region="deep"
      data-titlebar-inset={sidebarOpen ? undefined : inset}
      className="flex h-10 shrink-0 items-center gap-1 bg-background pr-2 pl-3 select-none"
      style={!sidebarOpen && inset ? { paddingLeft: inset } : undefined}
    >
      {!sidebarOpen && (
        <div className="mr-1 flex shrink-0 items-center">
          <SidebarToggle />
        </div>
      )}
      {active && <DatabaseTitle connectionId={active.connectionId} database={active.database} />}
      {active && <Tabs connectionId={active.connectionId} database={active.database} />}
      <div className="min-w-4 flex-1 self-stretch" />
      {active && (
        <ConnectionActions connectionId={active.connectionId} database={active.database} />
      )}
      {!rightPanelOpen && <RightPanelToggle />}
    </header>
  )
}

export function RightPanelToggle() {
  const open = useStore((s) => s.rightPanelOpen)
  return (
    <IconButton
      label={open ? 'Hide panel (⌥E)' : 'Show panel (⌥E)'}
      side="bottom"
      aria-pressed={open}
      className="shrink-0"
      onClick={() => useStore.setState((s) => ({ rightPanelOpen: !s.rightPanelOpen }))}
    >
      <PanelRight className="size-4" />
    </IconButton>
  )
}

/** Compact database selector: the database name; opens the ⌘K palette. */
function DatabaseTitle({ connectionId, database }: { connectionId: string; database: string }) {
  const conn = useStore((s) => s.connections[connectionId])
  if (!conn) return null
  const color = connectionColor(conn)
  const tooltip = [database, `${conn.name} · ${connectionString(conn)}`, 'Search (⌘K)'].join('\n')
  return (
    <button
      type="button"
      title={tooltip}
      aria-label={`Database ${database} on ${conn.name}. Search`}
      aria-haspopup="dialog"
      onClick={() => useStore.setState({ paletteOpen: true })}
      className="group -ml-1.5 flex h-7 max-w-[200px] min-w-0 shrink-0 cursor-pointer items-center gap-1.5 rounded-md px-1.5 text-quiet hover:bg-hover hover:text-foreground"
    >
      <Database
        className="size-3.5 shrink-0 text-dim group-hover:text-quiet"
        style={color ? { color } : undefined}
      />
      <span className="min-w-0 truncate font-mono text-sm">{database}</span>
      <ChevronDown className="size-3.5 shrink-0 text-dim" />
    </button>
  )
}

/** "…" menu: copy the connection URL, details, disconnect. */
function ConnectionActions({ connectionId, database }: { connectionId: string; database: string }) {
  const conn = useStore((s) => s.connections[connectionId])
  if (!conn) return null
  const url = connectionString(conn, database)
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        aria-label="Connection actions"
        className="grid size-6 shrink-0 cursor-pointer place-items-center rounded-md text-muted-foreground outline-none hover:bg-hover-strong hover:text-foreground data-[state=open]:bg-hover-strong"
      >
        <MoreHorizontal className="size-4" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" sideOffset={4} className="w-56">
        <DropdownMenuItem
          onSelect={() => {
            void navigator.clipboard?.writeText(url).catch(() => {})
            pushToast({ kind: 'info', title: `Copied ${url}` }, 3200)
          }}
        >
          <Copy />
          Copy connection URL
        </DropdownMenuItem>
        <DropdownMenuItem
          onSelect={() => {
            setPrefs({ rightTab: 'info' })
            useStore.setState({ rightPanelOpen: true })
          }}
        >
          <Info />
          Connection details
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          variant="destructive"
          disabled={conn.state !== 'connected'}
          onSelect={() => disconnect(connectionId)}
        >
          <Unplug />
          Disconnect
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

function Tabs({ connectionId, database }: { connectionId: string; database: string }) {
  const key = dbKey(connectionId, database)
  const ws = useStore((s) => s.workspaces[key])
  const connected = useStore((s) => s.connections[connectionId]?.state === 'connected')
  const running = useStore((s) => s.running)
  const tabs = ws?.tabs ?? []
  const activeTab = ws?.activeTab ?? null
  const stripRef = useRef<HTMLDivElement>(null)
  const drag = useRef<{ id: string; startX: number; moved: boolean } | null>(null)
  const [dragging, setDragging] = useState<string | null>(null)

  // Pointer events rather than HTML5 drag-and-drop: Tauri's native file-drop
  // handler swallows HTML5 drag events in the webview. The strip reorders
  // live as the pointer crosses a neighbour's midpoint.
  const onPointerMove = (e: ReactPointerEvent) => {
    const d = drag.current
    if (!d) return
    if (!d.moved) {
      if (Math.abs(e.clientX - d.startX) < 4) return
      d.moved = true
      setDragging(d.id)
    }
    const els = Array.from(stripRef.current?.querySelectorAll<HTMLElement>('[role="tab"]') ?? [])
    const from = els.findIndex((el) => el.dataset.tabId === d.id)
    if (from < 0) return
    const mid = (el: HTMLElement) => {
      const r = el.getBoundingClientRect()
      return r.left + r.width / 2
    }
    const next = els[from + 1]
    const prev = els[from - 1]
    if (next && e.clientX > mid(next)) moveTab(key, d.id, from + 1)
    else if (prev && e.clientX < mid(prev)) moveTab(key, d.id, from - 1)
  }
  const endDrag = (e: ReactPointerEvent) => {
    const d = drag.current
    if (!d) return
    drag.current = null
    setDragging(null)
    ;(e.currentTarget as HTMLElement).releasePointerCapture?.(e.pointerId)
    // A drag isn't a click: keep the trailing click from re-activating.
    if (d.moved) {
      const swallow = (ev: MouseEvent) => ev.stopPropagation()
      window.addEventListener('click', swallow, { capture: true, once: true })
      setTimeout(() => window.removeEventListener('click', swallow, { capture: true }), 0)
    }
  }

  return (
    <>
      <div
        ref={stripRef}
        className="no-scrollbar ml-2 flex min-w-0 items-center gap-0.5 overflow-x-auto"
        role="tablist"
      >
        {tabs.map((t) => {
          const id = t.id
          const isActive = id === activeTab
          const label = tabLabel(t)
          const isView =
            t.kind === 'table' &&
            findTable(connectionId, database, t.schema, t.table)?.kind === 'view'
          const Icon = t.kind === 'query' ? FileCode : isView ? Eye : Table2
          return (
            <div
              key={id}
              role="tab"
              aria-selected={isActive}
              data-active={isActive ? 'true' : 'false'}
              data-tab-id={id}
              title={t.kind === 'table' ? `${t.schema}.${t.table}` : t.title}
              onClick={() => activateTab(key, id)}
              onDoubleClick={(e) => {
                if (!(e.target as HTMLElement).closest('button')) pinTab(key, id)
              }}
              onPointerDown={(e) => {
                if (e.button !== 0 || (e.target as HTMLElement).closest('button')) return
                drag.current = { id, startX: e.clientX, moved: false }
                e.currentTarget.setPointerCapture(e.pointerId)
              }}
              onPointerMove={onPointerMove}
              onPointerUp={endDrag}
              onPointerCancel={endDrag}
              onMouseDown={(e) => {
                if (e.button === 1) e.preventDefault()
              }}
              onAuxClick={(e) => {
                if (e.button === 1) {
                  e.preventDefault()
                  closeTab(key, id)
                }
              }}
              className={cn(
                'group/tab relative flex h-7 shrink-0 cursor-default items-center gap-1.5 rounded-md pr-1 pl-2.5 text-base select-none',
                isActive ? 'bg-card text-foreground' : 'text-quiet hover:bg-hover hover:text-soft',
                dragging === id && 'z-10 cursor-grabbing bg-hover-strong text-foreground'
              )}
            >
              {id in running ? (
                <Spinner aria-label="Running" className="size-3.5 shrink-0 text-status-run" />
              ) : (
                <Icon className="size-3.5 shrink-0 text-quiet" />
              )}
              <span
                className={cn(
                  'whitespace-nowrap',
                  t.kind === 'table' && 'font-mono text-sm',
                  t.kind === 'table' && t.preview && 'italic'
                )}
              >
                {label}
              </span>
              <button
                type="button"
                aria-label={`Close tab ${label}`}
                onClick={(e) => {
                  e.stopPropagation()
                  closeTab(key, id)
                }}
                className={cn(
                  'grid size-[18px] cursor-pointer place-items-center rounded-[4px] text-quiet hover:bg-hover-strong hover:text-foreground',
                  !isActive && 'opacity-0 group-hover/tab:opacity-100'
                )}
              >
                <X className="size-3" />
              </button>
            </div>
          )
        })}
      </div>
      <DropdownMenu>
        <DropdownMenuTrigger
          title="New tab"
          aria-label="New tab"
          disabled={!connected}
          className="ml-0.5 grid size-6 shrink-0 cursor-pointer place-items-center rounded-md text-quiet outline-none hover:bg-hover-strong hover:text-foreground disabled:cursor-default disabled:opacity-40 data-[state=open]:bg-hover-strong"
        >
          <Plus className="size-4" />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" sideOffset={4} className="w-[200px]">
          <DropdownMenuItem onSelect={() => newQueryTab(connectionId, database)}>
            <FileCode />
            New query<DropdownMenuShortcut>⌘T</DropdownMenuShortcut>
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => useStore.setState({ paletteOpen: true })}>
            <Search />
            Open table…<DropdownMenuShortcut>⌘K</DropdownMenuShortcut>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </>
  )
}

// Connection sidebar, laid out like Cursor's: it runs the full window
// height, with the traffic lights, sidebar toggle and previous / next
// database arrows on top, then the nav (New query ⌥N, Search ⌘K, New
// connection ⌘O), then one section per connection (name and state; latency
// in the dot's tooltip) holding its databases and their tables: a table /
// view icon, `schema.table` (schema hidden for the default one) and the
// approximate row count. Settings at the bottom. Connections and databases
// collapse on a click of their header; the layout is saved in the prefs.
// Single-click opens a table as a preview tab, double-click pins it.

import { useState, type ReactNode } from 'react'
import {
  ArrowLeft,
  ArrowRight,
  ChevronDown,
  ChevronRight,
  Copy,
  Database,
  Eye,
  FileCode,
  MoreHorizontal,
  PanelLeft,
  Plug,
  Plus,
  Search,
  Settings,
  SquarePen,
  Table2,
  Unplug
} from 'lucide-react'
import {
  connect,
  connectionString,
  cycleDatabase,
  dbKey,
  disconnect,
  newQueryTab,
  openTable,
  pushToast,
  selectDatabase,
  useStore
} from '@/state/store'
import type { Connection, Database as Db } from '@/state/types'
import { applyOrder, clampSidebarWidth, DEFAULT_PREFS, setPrefs, usePrefs } from '@/state/prefs'
import { cn } from '@/lib/utils'
import { compactCount } from '@/lib/format'
import { useTitleBarInset } from '@/lib/window-chrome'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger
} from '@/components/ui/dropdown-menu'
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger
} from '@/components/ui/context-menu'
import { Kbd } from './chrome'
import { IconButton } from './IconButton'
import { PanelResizeHandle } from './PanelResizeHandle'

export function Sidebar() {
  const order = useStore((s) => s.connectionOrder)
  const connections = useStore((s) => s.connections)
  const savedOrder = usePrefs((s) => s.connectionOrder)
  const ids = applyOrder(
    order.filter((id) => connections[id]),
    savedOrder
  )
  const savedWidth = usePrefs((s) => s.sidebarWidth)
  // While dragging the width lives here; it is saved once, on release.
  const [dragWidth, setDragWidth] = useState<number | null>(null)
  const width = dragWidth ?? clampSidebarWidth(savedWidth)
  return (
    <aside
      className="relative flex h-full shrink-0 flex-col border-r border-border bg-sidebar text-sidebar-foreground"
      style={{ width }}
    >
      <PanelResizeHandle
        edge="right"
        label="Resize sidebar"
        width={width}
        clamp={clampSidebarWidth}
        onDrag={setDragWidth}
        onCommit={(w) => setPrefs({ sidebarWidth: w })}
        onReset={() => setPrefs({ sidebarWidth: DEFAULT_PREFS.sidebarWidth })}
      />
      <SidebarTop />
      <SidebarNav />
      <div className="min-h-0 flex-1 overflow-y-auto pt-3 pb-3" data-sidebar-scroll>
        {ids.map((id) => (
          <ConnectionSection key={id} conn={connections[id]!} />
        ))}
      </div>
      <SidebarFooter />
    </aside>
  )
}

/** Window-title row: traffic-light inset, sidebar toggle, database history arrows. */
function SidebarTop() {
  const inset = useTitleBarInset()
  return (
    <div
      data-tauri-drag-region="deep"
      data-titlebar-inset={inset}
      className="flex h-10 shrink-0 items-center gap-0.5 pr-2 pl-2 select-none"
      style={inset ? { paddingLeft: inset } : undefined}
    >
      <SidebarToggle />
      <span className="flex-1" />
      <IconButton label="Previous database (⌥↑)" side="bottom" onClick={() => cycleDatabase(-1)}>
        <ArrowLeft className="size-4" />
      </IconButton>
      <IconButton label="Next database (⌥↓)" side="bottom" onClick={() => cycleDatabase(1)}>
        <ArrowRight className="size-4" />
      </IconButton>
    </div>
  )
}

export function SidebarToggle() {
  const open = useStore((s) => s.sidebarOpen)
  return (
    <IconButton
      label={open ? 'Hide sidebar (⌥B)' : 'Show sidebar (⌥B)'}
      side="bottom"
      aria-pressed={open}
      onClick={() => useStore.setState((s) => ({ sidebarOpen: !s.sidebarOpen }))}
    >
      <PanelLeft className="size-4" />
    </IconButton>
  )
}

/** Where "New query" goes: the selected database, else the first connected one. */
export function newQueryTarget(): { connectionId: string; database: string } | null {
  const s = useStore.getState()
  if (s.active && s.connections[s.active.connectionId]?.state === 'connected') return s.active
  for (const id of s.connectionOrder) {
    const c = s.connections[id]
    if (c?.state === 'connected' && c.databases[0])
      return { connectionId: id, database: c.databases[0].name }
  }
  return null
}

function SidebarNav() {
  const anyConnected = useStore((s) =>
    s.connectionOrder.some((id) => s.connections[id]?.state === 'connected')
  )
  return (
    <nav className="flex shrink-0 flex-col gap-px px-2 pt-1">
      <NavItem
        icon={<SquarePen />}
        label="New query"
        hint="⌥N"
        disabled={!anyConnected}
        onClick={() => {
          const t = newQueryTarget()
          if (t) newQueryTab(t.connectionId, t.database)
        }}
      />
      <NavItem
        icon={<Search />}
        label="Search"
        hint="⌘K"
        onClick={() => useStore.setState({ paletteOpen: true })}
      />
      <NavItem
        icon={<Plug />}
        label="New connection"
        hint="⌘O"
        onClick={() => pushToast({ kind: 'info', title: 'New connection: not built yet' }, 2400)}
      />
    </nav>
  )
}

function NavItem({
  icon,
  label,
  hint,
  disabled,
  onClick
}: {
  icon: ReactNode
  label: string
  hint?: string
  disabled?: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onClick}
      className="group flex h-[30px] cursor-pointer items-center gap-2.5 rounded-md px-2 text-left text-base text-soft hover:bg-hover hover:text-foreground disabled:cursor-default disabled:opacity-50 disabled:hover:bg-transparent [&_svg]:size-4 [&_svg]:shrink-0 [&_svg]:text-quiet"
    >
      {icon}
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {hint && <Kbd className="opacity-0 transition-opacity group-hover:opacity-100">{hint}</Kbd>}
    </button>
  )
}

function SidebarFooter() {
  return (
    <div className="flex h-12 shrink-0 items-center gap-2 px-2">
      <button
        type="button"
        onClick={() => useStore.setState({ settings: 'general' })}
        className="flex h-[30px] min-w-0 flex-1 cursor-pointer items-center gap-2.5 rounded-md px-2 text-base text-soft hover:bg-hover hover:text-foreground"
        title="Settings (⌘,)"
      >
        <Settings className="size-4 shrink-0 text-quiet" />
        <span className="truncate">Settings</span>
      </button>
    </div>
  )
}

export function connMeta(c: Connection): string {
  switch (c.state) {
    case 'connected':
      return c.engine === 'sqlite'
        ? 'local'
        : c.latencyMs != null
          ? `${c.latencyMs} ms`
          : 'connected'
    case 'connecting':
      return 'connecting'
    case 'failed':
      return 'failed'
    default:
      return 'offline'
  }
}

export function ConnStateDot({ conn }: { conn: Connection }) {
  const s = conn.state
  const label = s === 'connected' ? `connected · ${connMeta(conn)}` : s
  return (
    <span
      role="img"
      aria-label={label}
      title={label}
      data-conn-state={s}
      className={cn(
        'inline-block size-1.5 shrink-0 rounded-full',
        s === 'connected' && 'bg-status-ok',
        s === 'connecting' && 'bg-status-wait',
        s === 'failed' && 'bg-status-err',
        s === 'disconnected' && 'border-[1.5px] border-dashed border-dim'
      )}
    />
  )
}

/** Small icon button that only shows while its row is hovered (or its menu is open). */
const ROW_ACTION =
  'grid size-6 shrink-0 cursor-pointer place-items-center rounded-md text-quiet opacity-0 group-hover:opacity-100 hover:bg-hover-strong hover:text-foreground focus-visible:opacity-100 data-[state=open]:opacity-100 disabled:cursor-default disabled:opacity-0 [&_svg]:size-3.5'

function ConnectionSection({ conn }: { conn: Connection }) {
  const collapsed = usePrefs((s) => !!s.collapsedConnections[conn.id])
  const connected = conn.state === 'connected'
  const toggle = () => {
    const cur = { ...usePrefs.getState().collapsedConnections }
    if (cur[conn.id]) delete cur[conn.id]
    else cur[conn.id] = true
    setPrefs({ collapsedConnections: cur })
  }
  const copy = () => {
    void navigator.clipboard?.writeText(connectionString(conn)).catch(() => {})
    pushToast({ kind: 'info', title: `Copied ${connectionString(conn)}` }, 3200)
  }
  return (
    <section className="relative mb-3">
      <div
        role="button"
        tabIndex={-1}
        aria-expanded={!collapsed}
        title={`${connectionString(conn)}\nClick to collapse`}
        onClick={toggle}
        className="group mx-2 flex h-7 cursor-default items-center gap-2 px-2"
      >
        <span className="truncate text-sm text-quiet">{conn.name}</span>
        <ConnStateDot conn={conn} />
        {collapsed && <ChevronRight aria-hidden className="size-3 shrink-0 text-dim" />}
        <span className="flex-1" />
        {!connected && (
          <span className="font-mono text-2xs text-dim group-hover:hidden">{connMeta(conn)}</span>
        )}
        {!connected && conn.state !== 'connecting' && (
          <button
            type="button"
            aria-label={`Connect ${conn.name}`}
            title="Connect"
            onClick={(e) => {
              e.stopPropagation()
              connect(conn.id)
            }}
            className={cn(ROW_ACTION, 'hidden group-hover:grid')}
          >
            <Plug />
          </button>
        )}
        <DropdownMenu>
          <DropdownMenuTrigger
            aria-label={`${conn.name} actions`}
            onClick={(e) => e.stopPropagation()}
            className={cn(ROW_ACTION, '-mr-1.5 hidden group-hover:grid data-[state=open]:grid')}
          >
            <MoreHorizontal />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-52">
            <DropdownMenuItem onSelect={copy}>
              <Copy />
              Copy connection URL
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            {connected ? (
              <DropdownMenuItem onSelect={() => disconnect(conn.id)}>
                <Unplug />
                Disconnect
              </DropdownMenuItem>
            ) : (
              <DropdownMenuItem
                disabled={conn.state === 'connecting'}
                onSelect={() => connect(conn.id)}
              >
                <Plug />
                Connect
              </DropdownMenuItem>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      {!collapsed && !connected && (
        <div className="mx-2 px-2 pb-1 text-2xs text-dim">
          {conn.state === 'connecting' ? 'Connecting…' : 'Not connected'}
        </div>
      )}
      {!collapsed &&
        connected &&
        conn.databases.map((db) => <DatabaseGroup key={db.name} conn={conn} db={db} />)}
    </section>
  )
}

function DatabaseGroup({ conn, db }: { conn: Connection; db: Db }) {
  const key = dbKey(conn.id, db.name)
  const collapsed = usePrefs((s) => !!s.collapsedDatabases[key])
  const selected = useStore(
    (s) => s.active?.connectionId === conn.id && s.active.database === db.name
  )
  const showViews = usePrefs((s) => s.sidebarViews)
  // The default schema's tables go unprefixed; others read `schema.table`.
  const defaultSchema = db.schemas[0]?.name
  const tables = db.schemas.flatMap((s) =>
    s.tables.filter((t) => showViews || t.kind === 'table').map((t) => ({ schema: s.name, t }))
  )
  const toggle = () => {
    const cur = { ...usePrefs.getState().collapsedDatabases }
    if (cur[key]) delete cur[key]
    else cur[key] = true
    setPrefs({ collapsedDatabases: cur })
  }
  return (
    <div className="relative">
      <div
        role="button"
        tabIndex={-1}
        aria-expanded={!collapsed}
        title="Click to collapse"
        onClick={() => {
          selectDatabase(conn.id, db.name)
          toggle()
        }}
        className={cn(
          'group mx-2 flex h-[30px] cursor-default items-center gap-2.5 rounded-md px-2 hover:bg-hover'
        )}
      >
        {/* Cursor's disclosure: an icon when collapsed, a chevron when open
            (and on hover, so the click target reads as a toggle). */}
        <span className="grid size-4 shrink-0 place-items-center text-quiet">
          {collapsed ? (
            <>
              <Database className="size-4 group-hover:hidden" />
              <ChevronRight className="hidden size-4 group-hover:block" />
            </>
          ) : (
            <ChevronDown className="size-4" />
          )}
        </span>
        <span
          className={cn(
            'min-w-0 flex-1 truncate text-base',
            selected ? 'text-foreground' : 'text-soft'
          )}
        >
          {db.name}
        </span>
        <button
          type="button"
          title={`New query in ${db.name} (⌥N)`}
          aria-label={`New query in ${db.name}`}
          onClick={(e) => {
            e.stopPropagation()
            newQueryTab(conn.id, db.name)
          }}
          className={cn(ROW_ACTION, '-mr-1')}
        >
          <Plus />
        </button>
      </div>
      {!collapsed && (
        <div className="mb-1 flex flex-col gap-px">
          {tables.map(({ schema, t }) => (
            <TableRow
              key={`${schema}.${t.name}`}
              conn={conn}
              db={db.name}
              schema={schema}
              name={t.name}
              view={t.kind === 'view'}
              rows={t.rows}
              prefix={schema !== defaultSchema}
            />
          ))}
          {tables.length === 0 && (
            <div className="mx-2 px-2 py-1 pl-[34px] text-sm text-dim">No tables</div>
          )}
        </div>
      )}
    </div>
  )
}

function TableRow({
  conn,
  db,
  schema,
  name,
  view,
  rows,
  prefix
}: {
  conn: Connection
  db: string
  schema: string
  name: string
  view: boolean
  rows: number
  prefix: boolean
}) {
  const key = dbKey(conn.id, db)
  const selected = useStore((s) => {
    if (s.active?.connectionId !== conn.id || s.active.database !== db) return false
    const ws = s.workspaces[key]
    const tab = ws?.tabs.find((t) => t.id === ws.activeTab)
    return tab?.kind === 'table' && tab.schema === schema && tab.table === name
  })
  const showCounts = usePrefs((s) => s.sidebarRowCounts)
  const Icon = view ? Eye : Table2
  const select = `SELECT * FROM ${prefix ? `${schema}.` : ''}${name} LIMIT 100;`
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          role="button"
          tabIndex={-1}
          data-active={selected ? 'true' : 'false'}
          title={`${schema}.${name}${view ? ' (view)' : ''}\n~${rows.toLocaleString()} rows`}
          onClick={() => openTable(conn.id, db, schema, name)}
          onDoubleClick={() => openTable(conn.id, db, schema, name, { pin: true })}
          className={cn(
            'group/row relative mx-2 flex h-[30px] cursor-default items-center gap-2.5 rounded-md px-2',
            selected ? 'bg-sidebar-accent' : 'hover:bg-hover'
          )}
        >
          <span className="grid size-4 shrink-0 place-items-center">
            <Icon className="size-3.5 text-dim" />
          </span>
          <span
            className={cn(
              'min-w-0 flex-1 truncate font-mono text-sm',
              selected ? 'text-foreground' : 'text-soft'
            )}
          >
            {prefix && <span className="text-dim">{schema}.</span>}
            {name}
          </span>
          {showCounts && <span className="shrink-0 text-sm text-quiet">{compactCount(rows)}</span>}
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent className="w-56">
        <ContextMenuItem onSelect={() => openTable(conn.id, db, schema, name, { pin: true })}>
          <Table2 />
          Open in new tab
        </ContextMenuItem>
        <ContextMenuItem onSelect={() => newQueryTab(conn.id, db, select)}>
          <FileCode />
          New query: SELECT *
        </ContextMenuItem>
        <ContextMenuItem
          onSelect={() => {
            void navigator.clipboard?.writeText(`${schema}.${name}`).catch(() => {})
            pushToast({ kind: 'info', title: `Copied ${schema}.${name}` }, 2400)
          }}
        >
          <Copy />
          Copy name
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  )
}

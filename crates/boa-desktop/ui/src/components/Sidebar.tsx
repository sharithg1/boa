// Database Explorer, after DataGrip's: a header row (traffic-light inset,
// sidebar toggle, title), a toolbar (new, refresh, console, collapse all,
// previous / next database, search) and a tree of connection › database ›
// schema › tables / views › table › columns (then keys, foreign keys,
// indexes and checks folders), each level with its child count. Click
// selects (a table opens as a preview tab, a database becomes
// the active one), double-click pins a table, the chevron or double-click
// expands. Expanding a disconnected connection connects it. Arrow keys move
// through the tree (→ expands / enters, ← collapses / goes to the parent,
// ↵ opens). Expansion is saved in the prefs.

import { useState, type KeyboardEvent, type ReactNode } from 'react'
import {
  ArrowLeft,
  ArrowRight,
  Boxes,
  ChevronDown,
  ChevronRight,
  ChevronsDownUp,
  Columns3,
  Copy,
  Pencil,
  Cylinder,
  Eye,
  FileCode,
  FileDown,
  Folder,
  KeyRound,
  ListOrdered,
  PanelLeft,
  Plug,
  Plus,
  RefreshCw,
  ScrollText,
  Search,
  Server,
  Settings,
  SquareCheck,
  SquareTerminal,
  Table2,
  Unplug
} from 'lucide-react'
import {
  connect,
  databaseSchemas,
  loadDatabase,
  schemaScopeKey,
  setScope,
  shownDatabases,
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
import type { Connection } from '@/state/types'
import { applyOrder, clampSidebarWidth, DEFAULT_PREFS, setPrefs, usePrefs } from '@/state/prefs'
import { cn } from '@/lib/utils'
import { compactCount } from '@/lib/format'
import { useTitleBarInset } from '@/lib/window-chrome'
import { qualified } from '@/lib/sql'
import { exportTable } from '@/state/csvExport'
import { openDdl } from '@/state/ddl'
import { columnKeys, metaFolders, type MetaFolderKind, type MetaItem } from '@/lib/tableMeta'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuShortcut,
  DropdownMenuTrigger
} from '@/components/ui/dropdown-menu'
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger
} from '@/components/ui/context-menu'
import { IconButton } from './IconButton'
import { PanelResizeHandle } from './PanelResizeHandle'
import { ScopePicker } from './ScopePicker'
import { ConnectionTag } from './ConnectionTag'
import { connectionColor } from '@/lib/connTag'

export function Sidebar() {
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
      <ExplorerToolbar />
      <Tree />
      <SidebarFooter />
    </aside>
  )
}

/** Window-title row: traffic-light inset, sidebar toggle, title. */
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
      <span className="ml-1 truncate text-sm font-medium text-foreground">Database Explorer</span>
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

const newConnection = () => useStore.setState({ connectionDialog: {} })

/** Re-reads the selected database's schemas (or connects its connection). */
function refresh() {
  const { active, connections } = useStore.getState()
  if (!active) return
  const c = connections[active.connectionId]
  if (c?.state === 'connected')
    void loadDatabase(active.connectionId, active.database, { force: true })
  else void connect(active.connectionId)
}

function ExplorerToolbar() {
  const anyConnected = useStore((s) =>
    s.connectionOrder.some((id) => s.connections[id]?.state === 'connected')
  )
  const console_ = () => {
    const t = newQueryTarget()
    if (t) newQueryTab(t.connectionId, t.database)
  }
  return (
    <div className="flex h-8 shrink-0 items-center gap-0.5 border-b border-border px-2">
      <DropdownMenu>
        <DropdownMenuTrigger
          aria-label="New"
          className="grid size-6 shrink-0 cursor-pointer place-items-center rounded-md text-muted-foreground outline-none hover:bg-hover-strong hover:text-foreground data-[state=open]:bg-hover-strong"
        >
          <Plus className="size-4" />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-56">
          <DropdownMenuItem disabled={!anyConnected} onSelect={console_}>
            <SquareTerminal />
            Query console<DropdownMenuShortcut>⌥N</DropdownMenuShortcut>
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={newConnection}>
            <Plug />
            Data source<DropdownMenuShortcut>⌘O</DropdownMenuShortcut>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <IconButton label="Refresh selected database" side="bottom" onClick={refresh}>
        <RefreshCw className="size-3.5" />
      </IconButton>
      <IconButton
        label="Query console (⌥N)"
        side="bottom"
        disabled={!anyConnected}
        onClick={console_}
      >
        <SquareTerminal className="size-3.5" />
      </IconButton>
      <span aria-hidden className="mx-1 h-4 w-px bg-border" />
      <IconButton label="Collapse all" side="bottom" onClick={() => setPrefs({ treeExpanded: {} })}>
        <ChevronsDownUp className="size-3.5" />
      </IconButton>
      <span className="flex-1" />
      <IconButton label="Previous database (⌥↑)" side="bottom" onClick={() => cycleDatabase(-1)}>
        <ArrowLeft className="size-3.5" />
      </IconButton>
      <IconButton label="Next database (⌥↓)" side="bottom" onClick={() => cycleDatabase(1)}>
        <ArrowRight className="size-3.5" />
      </IconButton>
      <IconButton
        label="Find table (⌘K)"
        side="bottom"
        onClick={() => useStore.setState({ paletteOpen: true })}
      >
        <Search className="size-3.5" />
      </IconButton>
    </div>
  )
}

function SidebarFooter() {
  return (
    <div className="flex h-11 shrink-0 items-center gap-2 border-t border-border px-2">
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

/**
 * Server icon in the connection's colour, with its state as a badge on the
 * corner: green connected, pulsing amber connecting, red failed, none (and a
 * dimmed icon) when offline. `cutout` is the surface colour behind it.
 */
export function ConnectionIcon({
  conn,
  cutout = 'ring-sidebar group-aria-selected:ring-sidebar-accent'
}: {
  conn: Connection
  cutout?: string
}) {
  const color = connectionColor(conn)
  const s = conn.state
  const label =
    s === 'connected'
      ? `Connected · ${connMeta(conn)}`
      : s === 'connecting'
        ? 'Connecting…'
        : s === 'reconnecting'
          ? 'Connection lost: reconnecting…'
          : s === 'failed'
            ? 'Connection failed: expand to retry'
            : 'Not connected'
  return (
    <span
      role="img"
      aria-label={label}
      title={label}
      data-conn-state={s}
      className="relative inline-grid shrink-0 place-items-center"
    >
      <Server
        className={cn(ICON, 'text-quiet', s === 'disconnected' && 'opacity-55')}
        style={color ? { color } : undefined}
      />
      {s !== 'disconnected' && (
        <span
          aria-hidden
          className={cn(
            'absolute -right-[3px] -bottom-[3px] size-[7px] rounded-full ring-[1.5px]',
            cutout,
            s === 'connected' && 'bg-status-ok',
            (s === 'connecting' || s === 'reconnecting') && 'animate-pulse bg-status-wait',
            s === 'failed' && 'bg-status-err'
          )}
        />
      )}
    </span>
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
    case 'reconnecting':
      return 'reconnecting'
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
        (s === 'connecting' || s === 'reconnecting') && 'bg-status-wait',
        s === 'failed' && 'bg-status-err',
        s === 'disconnected' && 'border-[1.5px] border-dashed border-dim'
      )}
    />
  )
}

// ── Tree ──────────────────────────────────────────────────────────────────

type Node = {
  /** Tree path: `conn/db/schema/#table/name`; the expansion key. */
  id: string
  parent: string | null
  depth: number
  icon: ReactNode
  label: ReactNode
  /** Dim text after the label (child count, row count, column type). */
  meta?: ReactNode
  expandable: boolean
  /** Highlighted as the open table / active database. */
  current?: boolean
  mono?: boolean
  italic?: boolean
  /** Click (and ↵). */
  onSelect?: () => void
  /** Double-click; defaults to toggling expansion. */
  onOpen?: () => void
  /** Called before expanding (connect on expand). */
  onExpand?: () => void
  menu?: ReactNode
}

const ICON = 'size-3.5 shrink-0'

function toggleNode(id: string, open?: boolean) {
  const cur = { ...usePrefs.getState().treeExpanded }
  const next = open ?? !cur[id]
  if (next) cur[id] = true
  else delete cur[id]
  setPrefs({ treeExpanded: cur })
}

function copy(text: string) {
  void navigator.clipboard?.writeText(text).catch(() => {})
  pushToast({ kind: 'info', title: `Copied ${text}` }, 2400)
}

/** The visible rows, depth first, honouring expansion. */
function useNodes(): Node[] {
  const order = useStore((s) => s.connectionOrder)
  const connections = useStore((s) => s.connections)
  const active = useStore((s) => s.active)
  const activeTable = useStore((s) => {
    if (!s.active) return null
    const ws = s.workspaces[dbKey(s.active.connectionId, s.active.database)]
    const t = ws?.tabs.find((x) => x.id === ws.activeTab)
    // The tab object itself: a stable reference, so the selector does not loop.
    return t?.kind === 'table' ? t : null
  })
  const savedOrder = usePrefs((s) => s.connectionOrder)
  const expanded = usePrefs((s) => s.treeExpanded)
  const showViews = usePrefs((s) => s.sidebarViews)
  const showCounts = usePrefs((s) => s.sidebarRowCounts)
  const scopes = usePrefs((s) => s.scopes)

  const out: Node[] = []
  const ids = applyOrder(
    order.filter((id) => connections[id]),
    savedOrder
  )
  for (const cid of ids) {
    const conn = connections[cid]!
    const connected = conn.state === 'connected'
    const open = !!expanded[cid] && connected
    out.push({
      id: cid,
      parent: null,
      depth: 0,
      icon: <ConnectionIcon conn={conn} />,
      label: (
        <span className="flex min-w-0 items-center gap-2">
          <span className="truncate">{conn.name}</span>
          <ConnectionTag conn={conn} />
        </span>
      ),
      meta: connected ? (
        <ScopePicker
          kind="databases"
          items={conn.databases.map((d) => d.name)}
          selected={shownDatabases(scopes, conn)}
          isAll={scopes[cid] === '*'}
          defaultItem={conn.database}
          onApply={(scope) => setScope(cid, scope)}
        />
      ) : conn.state === 'failed' ? (
        <span className="text-status-err-text">failed</span>
      ) : conn.state === 'reconnecting' ? (
        <span className="text-status-wait">reconnecting…</span>
      ) : undefined,
      expandable: true,
      onExpand: () => {
        if (conn.state !== 'connected' && conn.state !== 'connecting') connect(cid)
      },
      menu: (
        <>
          {connected ? (
            <ContextMenuItem onSelect={() => disconnect(cid)}>
              <Unplug />
              Disconnect
            </ContextMenuItem>
          ) : (
            <ContextMenuItem disabled={conn.state === 'connecting'} onSelect={() => connect(cid)}>
              <Plug />
              Connect
            </ContextMenuItem>
          )}
          <ContextMenuItem onSelect={() => copy(connectionString(conn))}>
            <Copy />
            Copy connection URL
          </ContextMenuItem>
          <ContextMenuSeparator />
          <ContextMenuItem onSelect={() => useStore.setState({ connectionDialog: { id: cid } })}>
            <Pencil />
            Edit…
          </ContextMenuItem>
        </>
      )
    })
    if (!expanded[cid]) continue
    if (!connected) {
      out.push({
        id: `${cid}/#status`,
        parent: cid,
        depth: 1,
        icon: null,
        label: (
          <span className="text-dim">
            {conn.state === 'connecting'
              ? 'Connecting…'
              : conn.state === 'reconnecting'
                ? 'Reconnecting…'
                : 'Not connected'}
          </span>
        ),
        expandable: false
      })
      continue
    }
    if (!open) continue
    const shown = new Set(shownDatabases(scopes, conn))
    if (shown.size === 0)
      out.push({
        id: `${cid}/#none`,
        parent: cid,
        depth: 1,
        icon: null,
        label: <span className="text-dim">No databases picked: use the “of” badge</span>,
        expandable: false
      })
    for (const db of conn.databases) {
      if (!shown.has(db.name)) continue
      const did = `${cid}/${db.name}`
      const isActive = active?.connectionId === cid && active.database === db.name
      out.push({
        id: did,
        parent: cid,
        depth: 1,
        icon: <Cylinder className={cn(ICON, isActive ? 'text-link' : 'text-quiet')} />,
        label: db.name,
        meta: db.schemaNames ? (
          <span className="flex items-center gap-1.5">
            <ScopePicker
              kind="schemas"
              items={db.schemaNames}
              selected={databaseSchemas(scopes, cid, db.name, db.schemaNames)}
              isAll={scopes[schemaScopeKey(cid, db.name)] === '*'}
              defaultItem={db.schemaNames.includes('public') ? 'public' : db.schemaNames[0]}
              onApply={(scope) => setScope(schemaScopeKey(cid, db.name), scope)}
            />
            {db.loading && 'loading…'}
          </span>
        ) : db.loading ? (
          'loading…'
        ) : undefined,
        expandable: true,
        current: isActive && !activeTable,
        onSelect: () => selectDatabase(cid, db.name),
        onExpand: () => void loadDatabase(cid, db.name),
        menu: (
          <>
            <ContextMenuItem onSelect={() => newQueryTab(cid, db.name)}>
              <SquareTerminal />
              New query console
            </ContextMenuItem>
            <ContextMenuItem onSelect={() => void loadDatabase(cid, db.name, { force: true })}>
              <RefreshCw />
              Refresh
            </ContextMenuItem>
          </>
        )
      })
      if (!expanded[did]) continue
      // Drop unpicked schemas at once; newly picked ones appear as they load.
      const picked = new Set(
        db.schemaNames ? databaseSchemas(scopes, cid, db.name, db.schemaNames) : []
      )
      for (const schema of db.schemas) {
        if (!picked.has(schema.name)) continue
        const sid = `${did}/${schema.name}`
        const groups = (['table', 'view'] as const)
          .map((kind) => ({ kind, items: schema.tables.filter((t) => t.kind === kind) }))
          .filter((g) => g.items.length && (g.kind === 'table' || showViews))
        out.push({
          id: sid,
          parent: did,
          depth: 2,
          icon: <Boxes className={cn(ICON, 'text-quiet')} />,
          label: schema.name,
          meta: `${schema.tables.length}`,
          expandable: groups.length > 0,
          onSelect: () => selectDatabase(cid, db.name)
        })
        if (!expanded[sid]) continue
        for (const g of groups) {
          const gid = `${sid}/#${g.kind}`
          out.push({
            id: gid,
            parent: sid,
            depth: 3,
            icon: <Folder className={cn(ICON, 'text-quiet')} />,
            label: g.kind === 'table' ? 'tables' : 'views',
            meta: `${g.items.length}`,
            expandable: true
          })
          if (!expanded[gid]) continue
          for (const t of g.items) {
            const tid = `${gid}/${t.name}`
            const isCur =
              isActive && activeTable?.schema === schema.name && activeTable.table === t.name
            const select = `select * from ${qualified(schema.name, t.name)};`
            out.push({
              id: tid,
              parent: gid,
              depth: 4,
              icon:
                t.kind === 'view' ? (
                  <Eye className={cn(ICON, 'text-dim')} />
                ) : (
                  <Table2 className={cn(ICON, 'text-dim')} />
                ),
              label: t.name,
              mono: true,
              italic: isCur && !!activeTable?.preview,
              meta: showCounts ? compactCount(t.rows) : undefined,
              expandable: true,
              current: isCur,
              onSelect: () => openTable(cid, db.name, schema.name, t.name),
              onOpen: () => openTable(cid, db.name, schema.name, t.name, { pin: true }),
              menu: (
                <>
                  <ContextMenuItem
                    onSelect={() => openTable(cid, db.name, schema.name, t.name, { pin: true })}
                  >
                    <Table2 />
                    Open in new tab
                  </ContextMenuItem>
                  <ContextMenuItem onSelect={() => newQueryTab(cid, db.name, select)}>
                    <FileCode />
                    New console: select *
                  </ContextMenuItem>
                  <ContextMenuItem onSelect={() => void openDdl(cid, db.name, schema.name, t.name)}>
                    <ScrollText />
                    Show DDL
                  </ContextMenuItem>
                  <ContextMenuItem
                    onSelect={() => void exportTable(cid, db.name, schema.name, t.name)}
                  >
                    <FileDown />
                    Export to CSV…
                  </ContextMenuItem>
                  <ContextMenuSeparator />
                  <ContextMenuItem onSelect={() => copy(`${schema.name}.${t.name}`)}>
                    <Copy />
                    Copy qualified name
                  </ContextMenuItem>
                </>
              )
            })
            if (!expanded[tid]) continue
            const keyOf = columnKeys(t)
            for (const c of t.columns) {
              const key = keyOf.get(c.name)
              out.push({
                id: `${tid}/${c.name}`,
                parent: tid,
                depth: 5,
                icon: key ? (
                  <KeyRound
                    className={cn(ICON, key === 'primary' ? 'text-status-wait' : 'text-link')}
                    aria-label={key === 'primary' ? 'primary key' : 'foreign key'}
                  />
                ) : (
                  <Columns3 className={cn(ICON, 'text-dim')} />
                ),
                label: c.name,
                mono: true,
                meta: (
                  <span className="font-mono">
                    <span className="text-sql-type">{c.type}</span>
                    {c.nullable ? '' : ' not null'}
                  </span>
                ),
                expandable: false,
                menu: (
                  <ContextMenuItem onSelect={() => copy(c.name)}>
                    <Copy />
                    Copy name
                  </ContextMenuItem>
                )
              })
            }
            // DataGrip's folders after the columns: keys, foreign keys, indexes, checks.
            for (const f of metaFolders(schema.name, t)) {
              const fid = `${tid}/#${f.kind}`
              out.push({
                id: fid,
                parent: tid,
                depth: 5,
                icon: <Folder className={cn(ICON, 'text-quiet')} />,
                label: f.kind,
                meta: `${f.items.length}`,
                expandable: true
              })
              if (!expanded[fid]) continue
              for (const it of f.items) {
                out.push({
                  id: `${fid}/${it.name}`,
                  parent: fid,
                  depth: 6,
                  icon: <MetaIcon folder={f.kind} item={it} />,
                  label: it.name,
                  mono: true,
                  meta: <span className="font-mono">{it.detail}</span>,
                  expandable: false,
                  menu: (
                    <>
                      <ContextMenuItem onSelect={() => copy(it.name)}>
                        <Copy />
                        Copy name
                      </ContextMenuItem>
                      <ContextMenuItem onSelect={() => copy(it.definition)}>
                        <FileCode />
                        Copy definition
                      </ContextMenuItem>
                    </>
                  )
                })
              }
            }
          }
        }
      }
    }
  }
  return out
}

/** A key (primary / unique / foreign, DataGrip's colours), an index or a check. */
function MetaIcon({ folder, item }: { folder: MetaFolderKind; item: MetaItem }) {
  if (folder === 'checks') return <SquareCheck className={cn(ICON, 'text-dim')} />
  if (folder === 'indexes')
    return (
      <ListOrdered className={cn(ICON, item.key === 'primary' ? 'text-status-wait' : 'text-dim')} />
    )
  return (
    <KeyRound
      className={cn(
        ICON,
        item.key === 'primary'
          ? 'text-status-wait'
          : item.key === 'foreign'
            ? 'text-link'
            : 'text-dim'
      )}
    />
  )
}

function Tree() {
  const nodes = useNodes()
  const expanded = usePrefs((s) => s.treeExpanded)
  const [focus, setFocus] = useState<string | null>(null)

  const expand = (n: Node, open: boolean) => {
    if (!n.expandable) return
    if (open) n.onExpand?.()
    toggleNode(n.id, open)
  }

  const onKeyDown = (e: KeyboardEvent) => {
    const i = nodes.findIndex((n) => n.id === focus)
    const n = nodes[i]
    const go = (id: string | undefined) => {
      if (!id) return
      setFocus(id)
      document
        .querySelector(`[data-node-id="${CSS.escape(id)}"]`)
        ?.scrollIntoView({ block: 'nearest' })
    }
    switch (e.key) {
      case 'ArrowDown':
        go(nodes[Math.min(nodes.length - 1, i + 1)]?.id)
        break
      case 'ArrowUp':
        go(nodes[Math.max(0, i - 1)]?.id)
        break
      case 'ArrowRight':
        if (!n) return go(nodes[0]?.id)
        if (n.expandable && !expanded[n.id]) expand(n, true)
        else if (nodes[i + 1]?.parent === n.id) go(nodes[i + 1]!.id)
        break
      case 'ArrowLeft':
        if (!n) return
        if (n.expandable && expanded[n.id]) expand(n, false)
        else go(n.parent ?? undefined)
        break
      case 'Enter':
        if (!n) return
        if (n.onOpen) n.onOpen()
        else if (n.onSelect) n.onSelect()
        else expand(n, !expanded[n.id])
        break
      default:
        return
    }
    e.preventDefault()
  }

  return (
    <div
      role="tree"
      aria-label="Database Explorer"
      tabIndex={0}
      onKeyDown={onKeyDown}
      className="min-h-0 flex-1 overflow-auto py-1.5 outline-none"
      data-sidebar-scroll
    >
      {nodes.length === 0 && (
        <div className="flex flex-col items-center gap-2 px-4 py-8 text-center text-sm text-quiet">
          No data sources
          <button
            type="button"
            onClick={newConnection}
            className="flex h-7 cursor-pointer items-center gap-1.5 rounded-md border border-input bg-card px-2.5 text-soft hover:bg-hover-strong hover:text-foreground"
          >
            <Plus className="size-3.5" />
            Add PostgreSQL
          </button>
        </div>
      )}
      {nodes.map((n) => (
        <TreeRow
          key={n.id}
          node={n}
          open={!!expanded[n.id]}
          focused={focus === n.id}
          onFocus={() => setFocus(n.id)}
          onToggle={() => expand(n, !expanded[n.id])}
        />
      ))}
    </div>
  )
}

function TreeRow({
  node: n,
  open,
  focused,
  onFocus,
  onToggle
}: {
  node: Node
  open: boolean
  focused: boolean
  onFocus: () => void
  onToggle: () => void
}) {
  const row = (
    <div
      role="treeitem"
      aria-expanded={n.expandable ? open : undefined}
      aria-selected={focused}
      data-node-id={n.id}
      onClick={() => {
        onFocus()
        n.onSelect?.()
      }}
      onDoubleClick={() => (n.onOpen ? n.onOpen() : onToggle())}
      className={cn(
        'group mx-1.5 flex h-[26px] min-w-max cursor-default items-center gap-1.5 rounded-md pr-2 text-sm whitespace-nowrap',
        focused ? 'bg-sidebar-accent text-foreground' : 'hover:bg-hover',
        n.current && !focused && 'bg-hover text-foreground',
        !focused && !n.current && 'text-soft'
      )}
      style={{ paddingLeft: 6 + n.depth * 16 }}
    >
      <span
        aria-hidden
        onClick={(e) => {
          if (!n.expandable) return
          e.stopPropagation()
          onToggle()
        }}
        className="grid size-4 shrink-0 place-items-center text-dim hover:text-foreground"
      >
        {n.expandable &&
          (open ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />)}
      </span>
      {n.icon}
      <span className={cn('min-w-0', n.mono && 'font-mono', n.italic && 'italic')}>{n.label}</span>
      {n.meta != null && n.meta !== '' && (
        <span className="shrink-0 text-xs text-dim">{n.meta}</span>
      )}
    </div>
  )
  if (!n.menu) return row
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>{row}</ContextMenuTrigger>
      <ContextMenuContent className="w-56">{n.menu}</ContextMenuContent>
    </ContextMenu>
  )
}

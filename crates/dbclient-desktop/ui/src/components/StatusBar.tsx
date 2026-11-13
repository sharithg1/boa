// Full-width footer, after DataGrip's status bar: the Database Output toggle,
// breadcrumbs of the active tab (connection › database › schema › table, or
// the console), and on the right the caret position / row estimate and the
// active connection + latency (click for the connection popover).
import { ChevronDown, ChevronRight, PanelBottom, Server } from 'lucide-react'
import { connect, connectionString, dbKey, disconnect, findTable, useStore } from '@/state/store'
import { ENGINE_LABEL } from '@/lib/format'
import { cn } from '@/lib/utils'
import { IconButton } from './IconButton'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { ConnStateDot, connMeta } from './Sidebar'

export function StatusBar() {
  const active = useStore((s) => s.active)
  const order = useStore((s) => s.connectionOrder)
  const connections = useStore((s) => s.connections)
  const bottomOpen = useStore((s) => s.bottomOpen)
  const cursor = useStore((s) => s.cursor)
  const connId = active?.connectionId ?? order[0]
  const conn = connId ? connections[connId] : undefined
  const tab = useStore((s) => {
    if (!s.active) return undefined
    const ws = s.workspaces[dbKey(s.active.connectionId, s.active.database)]
    return ws?.tabs.find((t) => t.id === ws.activeTab)
  })
  // DataGrip-style breadcrumbs: connection › database › schema › table / console.
  const crumbs: string[] = []
  if (active) {
    crumbs.push(connections[active.connectionId]?.name ?? '', active.database)
    if (tab?.kind === 'table') crumbs.push(tab.schema, tab.table)
    else if (tab?.kind === 'query') crumbs.push(tab.title)
  }
  let right = ''
  if (active && tab?.kind === 'table') {
    const t = findTable(active.connectionId, active.database, tab.schema, tab.table)
    right = t ? `~${t.rows.toLocaleString()} rows` : ''
  } else if (tab?.kind === 'query' && cursor) {
    right = `${cursor.line}:${cursor.col}`
  }
  return (
    <footer className="flex h-8 shrink-0 items-center gap-1 overflow-hidden border-t border-border bg-background px-2 text-sm whitespace-nowrap text-quiet">
      <IconButton
        label={bottomOpen ? 'Hide Database Output (⌥J)' : 'Show Database Output (⌥J)'}
        side="top"
        aria-pressed={bottomOpen}
        className={cn(bottomOpen && 'text-foreground')}
        onClick={() => useStore.setState((s) => ({ bottomOpen: !s.bottomOpen }))}
      >
        <PanelBottom className="size-4" />
      </IconButton>
      <nav aria-label="Breadcrumbs" className="flex min-w-0 items-center gap-1 px-1">
        {crumbs.map((c, i) => (
          <span key={`${i}-${c}`} className="flex min-w-0 items-center gap-1">
            {i > 0 && <ChevronRight className="size-3 shrink-0 text-dim" />}
            <span
              className={cn(
                'truncate',
                i === crumbs.length - 1 ? 'text-soft' : 'text-quiet',
                i > 0 && 'font-mono text-xs'
              )}
            >
              {c}
            </span>
          </span>
        ))}
        {!active && <span className="text-dim">No database selected</span>}
      </nav>
      <span className="flex-1" />
      {right && <span className="truncate px-1.5 font-mono text-xs text-dim">{right}</span>}
      {tab?.kind === 'query' && <span className="px-1.5 text-xs text-dim">UTF-8</span>}
      {conn && connId && <ConnectionPopover connectionId={connId} />}
      {!conn && <span className="px-1.5">no connection</span>}
    </footer>
  )
}

function ConnectionPopover({ connectionId }: { connectionId: string }) {
  const conn = useStore((s) => s.connections[connectionId])!
  const btn =
    'h-7 cursor-pointer rounded-md border border-input bg-card text-sm text-soft hover:bg-hover-strong hover:text-foreground disabled:cursor-default disabled:opacity-50'
  return (
    <Popover>
      <PopoverTrigger className="flex h-6 cursor-pointer items-center gap-1.5 rounded-md px-1.5 outline-none hover:bg-hover hover:text-foreground data-[state=open]:bg-hover">
        <Server className="size-3.5 shrink-0" />
        {conn.name}
        <ConnStateDot conn={conn} />
        <span className="text-xs text-dim">{connMeta(conn)}</span>
        <ChevronDown className="size-3 shrink-0" />
      </PopoverTrigger>
      <PopoverContent
        side="top"
        align="end"
        sideOffset={6}
        className="flex w-[280px] flex-col gap-2 px-3.5 py-3 text-xs"
      >
        <div className="font-mono text-foreground">
          {conn.name} · {conn.state}
        </div>
        <div className="leading-normal break-all text-muted-foreground">
          {ENGINE_LABEL[conn.engine]}
          {conn.serverVersion ? ` · ${conn.serverVersion}` : ''}
          {conn.latencyMs != null && conn.engine !== 'sqlite' ? ` · ${conn.latencyMs} ms` : ''}
          <br />
          {connectionString(conn)}
        </div>
        {conn.state === 'connected' ? (
          <button type="button" className={btn} onClick={() => disconnect(connectionId)}>
            Disconnect
          </button>
        ) : (
          <button
            type="button"
            className={btn}
            disabled={conn.state === 'connecting'}
            onClick={() => connect(connectionId)}
          >
            {conn.state === 'connecting' ? 'Connecting…' : 'Connect'}
          </button>
        )}
      </PopoverContent>
    </Popover>
  )
}

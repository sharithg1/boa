// Footer of the main column, like the line under Cursor's composer: the
// selected database, the active connection + latency (click for the
// connection popover) and, on the right, what the active tab shows.
import { ChevronDown, Database, Server } from 'lucide-react'
import {
  connect,
  connectionString,
  dbKey,
  disconnect,
  findTable,
  simulateDrop,
  useStore
} from '@/state/store'
import { ENGINE_LABEL } from '@/lib/format'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { ConnStateDot, connMeta } from './Sidebar'

export function StatusBar() {
  const active = useStore((s) => s.active)
  const order = useStore((s) => s.connectionOrder)
  const connections = useStore((s) => s.connections)
  const connId = active?.connectionId ?? order[0]
  const conn = connId ? connections[connId] : undefined
  const tab = useStore((s) => {
    if (!s.active) return undefined
    const ws = s.workspaces[dbKey(s.active.connectionId, s.active.database)]
    return ws?.tabs.find((t) => t.id === ws.activeTab)
  })
  const connectedCount = order.filter((id) => connections[id]?.state === 'connected').length
  let right = `${connectedCount} connection${connectedCount === 1 ? '' : 's'} open`
  if (active && tab?.kind === 'table') {
    const t = findTable(active.connectionId, active.database, tab.schema, tab.table)
    right = `${tab.schema}.${tab.table}${t ? ` · ~${t.rows.toLocaleString()} rows` : ''}`
  } else if (tab?.kind === 'query') {
    const lines = tab.sql ? tab.sql.split('\n').length : 0
    right = `${tab.title} · ${lines} line${lines === 1 ? '' : 's'}`
  }
  return (
    <footer className="flex h-8 shrink-0 items-center gap-1 overflow-hidden bg-background px-2 text-sm whitespace-nowrap text-quiet">
      {active && (
        <span className="flex h-6 min-w-0 items-center gap-1.5 px-1.5">
          <Database className="size-3.5 shrink-0" />
          <span className="truncate font-mono text-xs text-soft">{active.database}</span>
        </span>
      )}
      {conn && connId && <ConnectionPopover connectionId={connId} />}
      {!conn && <span className="px-1.5">no connection</span>}
      <span className="flex-1" />
      <span className="truncate px-1.5 text-dim">{right}</span>
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
        align="start"
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
        <button
          type="button"
          className={btn}
          disabled={conn.state !== 'connected'}
          onClick={() => simulateDrop(connectionId)}
        >
          Simulate connection drop (mock)
        </button>
      </PopoverContent>
    </Popover>
  )
}

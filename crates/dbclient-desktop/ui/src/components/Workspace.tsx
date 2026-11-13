// Main area for the selected database: the active tab's contents (tabs live
// in the TitleBar). A query tab is a SQL editor above a results area; a table
// tab is the table's column header strip above the data grid. Both are shells
// for now: running a query is simulated and the grid has no rows yet.
import { Play, Table2 } from 'lucide-react'
import {
  dbKey,
  findTable,
  runQuery,
  setQuerySql,
  useStore,
  type QueryTab,
  type TableTab
} from '@/state/store'
import { Kbd } from './chrome'

export function Workspace() {
  const active = useStore((s) => s.active)
  const key = active ? dbKey(active.connectionId, active.database) : ''
  const ws = useStore((s) => (key ? s.workspaces[key] : undefined))

  if (!active) {
    return (
      <main className="grid min-h-0 min-w-0 flex-1 place-items-center bg-background text-sm text-quiet">
        <div className="flex flex-col items-center gap-2">
          Select a database
          <span className="flex items-center gap-1.5 text-dim">
            <Kbd>⌘K</Kbd> to search tables
          </span>
        </div>
      </main>
    )
  }
  const tab = ws?.tabs.find((t) => t.id === ws?.activeTab)
  return (
    <main className="flex min-h-0 min-w-0 flex-1 flex-col bg-background">
      <div className="relative min-h-0 flex-1">
        {tab?.kind === 'query' ? (
          <QueryView
            key={tab.id}
            connectionId={active.connectionId}
            database={active.database}
            tab={tab}
          />
        ) : tab?.kind === 'table' ? (
          <TableView
            key={tab.id}
            connectionId={active.connectionId}
            database={active.database}
            tab={tab}
          />
        ) : (
          <div className="grid h-full place-items-center text-sm text-quiet">
            <div className="flex flex-col items-center gap-2">
              No tabs open
              <span className="flex items-center gap-1.5 text-dim">
                <Kbd>⌘T</Kbd> new query · <Kbd>⌘K</Kbd> open a table
              </span>
            </div>
          </div>
        )}
      </div>
    </main>
  )
}

function QueryView({
  connectionId,
  database,
  tab
}: {
  connectionId: string
  database: string
  tab: QueryTab
}) {
  const key = dbKey(connectionId, database)
  const connected = useStore((s) => s.connections[connectionId]?.state === 'connected')
  const run = () => runQuery(connectionId, database, tab.sql)
  return (
    <div className="flex h-full flex-col">
      <div className="relative min-h-[120px] flex-[3] border-b border-border bg-[#1b1c1c]">
        <textarea
          autoFocus
          value={tab.sql}
          spellCheck={false}
          aria-label="SQL"
          placeholder={`-- ${database}\nSELECT …`}
          onChange={(e) => setQuerySql(key, tab.id, e.target.value)}
          onKeyDown={(e) => {
            if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
              e.preventDefault()
              run()
            }
          }}
          className="h-full w-full resize-none bg-transparent py-2.5 pr-3 pl-3.5 font-mono text-sm text-body outline-none placeholder:text-dim"
        />
        <button
          type="button"
          disabled={!connected || !tab.sql.trim()}
          onClick={run}
          title="Run (⌘↵)"
          className="absolute right-3 bottom-3 flex h-7 cursor-pointer items-center gap-1.5 rounded-md bg-primary px-2.5 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:cursor-default disabled:opacity-40"
        >
          <Play className="size-3.5" />
          Run
          <span className="font-mono text-2xs opacity-60">⌘↵</span>
        </button>
      </div>
      <div className="grid min-h-[80px] flex-[2] place-items-center text-sm text-quiet">
        <div className="flex flex-col items-center gap-2">
          Results appear here
          <span className="text-dim">Query execution is simulated until the backend lands</span>
        </div>
      </div>
    </div>
  )
}

function TableView({
  connectionId,
  database,
  tab
}: {
  connectionId: string
  database: string
  tab: TableTab
}) {
  useStore((s) => s.connections[connectionId])
  const table = findTable(connectionId, database, tab.schema, tab.table)
  if (!table) {
    return <div className="grid h-full place-items-center text-sm text-quiet">Table not found</div>
  }
  return (
    <div className="flex h-full flex-col overflow-hidden">
      <div className="overflow-x-auto border-b border-border">
        <div className="flex min-w-max">
          <div className="w-12 shrink-0 border-r border-border" />
          {table.columns.map((c) => (
            <div
              key={c.name}
              className="flex h-8 w-[180px] shrink-0 items-center gap-2 border-r border-border px-2.5 text-sm"
            >
              <span className="min-w-0 truncate font-mono text-soft">{c.name}</span>
              <span className="ml-auto shrink-0 font-mono text-2xs text-dim">{c.type}</span>
            </div>
          ))}
        </div>
      </div>
      <div className="grid flex-1 place-items-center text-sm text-quiet">
        <div className="flex flex-col items-center gap-3">
          <Table2 className="size-5 text-dim" />
          <div>
            {tab.schema}.{tab.table} · ~{table.rows.toLocaleString()} rows
          </div>
          <span className="text-dim">The data grid is not built yet</span>
        </div>
      </div>
    </div>
  )
}

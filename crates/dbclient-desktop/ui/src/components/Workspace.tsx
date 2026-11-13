// Editor area for the selected database (tabs live in the TitleBar), laid
// out like DataGrip: a query console is a toolbar (run / stop, transaction
// mode, history, schema selector) above a SQL editor with a line-number
// gutter; its results go to the bottom Database Output panel. A table tab is
// the table's data editor: a toolbar (refresh, filter, DDL, console) above
// the data grid. Execution is simulated (state/mockData.ts).
import { useMemo, useRef, useState, type ReactNode } from 'react'
import {
  Check,
  ChevronDown,
  FileCode,
  History,
  Play,
  RefreshCw,
  Search,
  Square,
  Undo2
} from 'lucide-react'
import {
  cancelQuery,
  dbKey,
  findTable,
  newQueryTab,
  pushToast,
  runQuery,
  setQuerySql,
  useStore,
  type QueryTab,
  type TableTab
} from '@/state/store'
import { setPrefs, usePrefs } from '@/state/prefs'
import { sampleRows, statementAt, tableColumns } from '@/state/mockData'
import type { Table } from '@/state/types'
import { cn } from '@/lib/utils'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger
} from '@/components/ui/dropdown-menu'
import { Kbd } from './chrome'
import { DataGrid } from './DataGrid'
import { IconButton } from './IconButton'

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
      <div className="relative flex min-h-0 flex-1 flex-col">
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
                <Kbd>⌘T</Kbd> new console · <Kbd>⌘K</Kbd> open a table
              </span>
            </div>
          </div>
        )}
      </div>
    </main>
  )
}

/** Toolbar row above an editor or grid. */
function Toolbar({ children }: { children: ReactNode }) {
  return (
    <div className="flex h-9 shrink-0 items-center gap-0.5 border-b border-border px-2">
      {children}
    </div>
  )
}

const Sep = () => <span aria-hidden className="mx-1.5 h-4 w-px shrink-0 bg-border" />

const TOOL_BTN =
  'flex h-6 shrink-0 cursor-pointer items-center gap-1 rounded-md px-1.5 text-xs text-quiet outline-none hover:bg-hover-strong hover:text-foreground disabled:cursor-default disabled:opacity-40 data-[state=open]:bg-hover-strong'

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
  const running = useStore((s) => !!s.running[tab.id])
  const db = useStore((s) =>
    s.connections[connectionId]?.databases.find((d) => d.name === database)
  )
  const [schema, setSchema] = useState(db?.schemas[0]?.name ?? '')
  const [manualTx, setManualTx] = useState(false)
  const area = useRef<HTMLTextAreaElement>(null)
  const gutter = useRef<HTMLDivElement>(null)
  const lines = tab.sql.split('\n').length

  const exec = (sql: string) => runQuery(connectionId, database, tab.id, tab.title, sql)
  /** Runs the selection, else the statement under the caret. */
  const run = () => {
    const el = area.current
    if (!el) return exec(tab.sql)
    const selected = tab.sql.slice(el.selectionStart, el.selectionEnd)
    exec(selected.trim() ? selected : statementAt(tab.sql, el.selectionStart))
  }
  const trackCaret = () => {
    const el = area.current
    if (!el) return
    const before = tab.sql.slice(0, el.selectionStart).split('\n')
    useStore.setState({
      cursor: { line: before.length, col: before[before.length - 1]!.length + 1 }
    })
  }

  return (
    <div className="flex h-full flex-col">
      <Toolbar>
        <IconButton
          label="Execute (⌘↵)"
          side="bottom"
          disabled={!connected || !tab.sql.trim() || running}
          onClick={run}
          className="text-status-ok hover:text-status-ok"
        >
          <Play className="size-3.5" />
        </IconButton>
        <IconButton
          label="Cancel running statement"
          side="bottom"
          disabled={!running}
          onClick={() => cancelQuery(tab.id)}
        >
          <Square className="size-3.5" />
        </IconButton>
        <IconButton
          label="Query history"
          side="bottom"
          onClick={() => {
            setPrefs({ rightTab: 'history' })
            useStore.setState({ rightPanelOpen: true })
          }}
        >
          <History className="size-3.5" />
        </IconButton>
        <Sep />
        <DropdownMenu>
          <DropdownMenuTrigger className={TOOL_BTN}>
            Tx: {manualTx ? 'Manual' : 'Auto'}
            <ChevronDown className="size-3" />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="w-44">
            <DropdownMenuItem onSelect={() => setManualTx(false)}>
              {!manualTx ? <Check /> : <span className="size-4" />}
              Auto-commit
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => setManualTx(true)}>
              {manualTx ? <Check /> : <span className="size-4" />}
              Manual
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        {manualTx && (
          <>
            <IconButton
              label="Commit"
              side="bottom"
              disabled={!connected}
              onClick={() => exec('COMMIT')}
            >
              <Check className="size-3.5" />
            </IconButton>
            <IconButton
              label="Rollback"
              side="bottom"
              disabled={!connected}
              onClick={() => exec('ROLLBACK')}
            >
              <Undo2 className="size-3.5" />
            </IconButton>
          </>
        )}
        {running && <span className="ml-2 text-xs text-dim">Executing…</span>}
        <span className="flex-1" />
        <DropdownMenu>
          <DropdownMenuTrigger className={cn(TOOL_BTN, 'font-mono')} title="Default schema">
            {database}.{schema}
            <ChevronDown className="size-3" />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-52">
            <DropdownMenuLabel className="text-xs text-dim">{database}</DropdownMenuLabel>
            <DropdownMenuSeparator />
            {db?.schemas.map((s) => (
              <DropdownMenuItem
                key={s.name}
                onSelect={() => setSchema(s.name)}
                className="font-mono"
              >
                {s.name === schema ? <Check /> : <span className="size-4" />}
                {s.name}
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      </Toolbar>
      <div className="flex min-h-0 flex-1 bg-[#1b1c1c]">
        <div
          ref={gutter}
          aria-hidden
          className="no-scrollbar w-12 shrink-0 overflow-hidden border-r border-border py-2.5 pr-2.5 text-right font-mono text-sm text-dim select-none"
        >
          {Array.from({ length: lines }, (_, i) => (
            <div key={i}>{i + 1}</div>
          ))}
        </div>
        <textarea
          ref={area}
          autoFocus
          value={tab.sql}
          spellCheck={false}
          aria-label="SQL"
          placeholder={`-- ${database}.${schema}\nSELECT …`}
          onChange={(e) => {
            setQuerySql(key, tab.id, e.target.value)
            trackCaret()
          }}
          onSelect={trackCaret}
          onFocus={trackCaret}
          onBlur={() => useStore.setState({ cursor: null })}
          onScroll={(e) => {
            if (gutter.current) gutter.current.scrollTop = e.currentTarget.scrollTop
          }}
          onKeyDown={(e) => {
            if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
              e.preventDefault()
              run()
            } else if (e.key === 'Tab' && !e.shiftKey) {
              // Indent rather than leave the editor.
              e.preventDefault()
              const el = e.currentTarget
              const { selectionStart: a, selectionEnd: b } = el
              setQuerySql(key, tab.id, `${tab.sql.slice(0, a)}    ${tab.sql.slice(b)}`)
              requestAnimationFrame(() => el.setSelectionRange(a + 4, a + 4))
            }
          }}
          className="h-full min-w-0 flex-1 resize-none bg-transparent py-2.5 pr-3 pl-3 font-mono text-sm whitespace-pre text-body outline-none placeholder:text-dim"
        />
      </div>
    </div>
  )
}

function ddl(schema: string, t: Table): string {
  const cols = t.columns.map(
    (c) =>
      `    ${c.name} ${c.type}${c.nullable ? '' : ' not null'}${c.default ? ` default ${c.default}` : ''}`
  )
  const pk = t.columns.filter((c) => c.pk).map((c) => c.name)
  if (pk.length) cols.push(`    primary key (${pk.join(', ')})`)
  return t.kind === 'view'
    ? `create view ${schema}.${t.name} as\n    select …;\n`
    : `create table ${schema}.${t.name}\n(\n${cols.join(',\n')}\n);\n`
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
  const rowLimit = usePrefs((s) => s.rowLimit)
  const [nonce, setNonce] = useState(0)
  const [filter, setFilter] = useState('')
  const table = findTable(connectionId, database, tab.schema, tab.table)
  const columns = useMemo(() => (table ? tableColumns(table) : []), [table])
  // biome-ignore lint/correctness/useExhaustiveDependencies: nonce forces a reload
  const rows = useMemo(() => (table ? sampleRows(table, rowLimit) : []), [table, rowLimit, nonce])
  const shown = useMemo(() => {
    const q = filter.trim().toLowerCase()
    return q ? rows.filter((r) => r.some((v) => v?.toLowerCase().includes(q))) : rows
  }, [rows, filter])
  if (!table) {
    return <div className="grid h-full place-items-center text-sm text-quiet">Table not found</div>
  }
  const select = `select * from ${tab.schema}.${tab.table};`
  return (
    <div className="flex h-full flex-col overflow-hidden">
      <Toolbar>
        <IconButton
          label="Reload page"
          side="bottom"
          onClick={() => {
            setNonce((n) => n + 1)
            pushToast({ kind: 'info', title: `Reloaded ${tab.table}` }, 1600)
          }}
        >
          <RefreshCw className="size-3.5" />
        </IconButton>
        <span className="px-1.5 text-xs text-dim">
          {Math.min(rowLimit, table.rows).toLocaleString()} of ~{table.rows.toLocaleString()}
        </span>
        <Sep />
        <label className="flex h-6 min-w-0 max-w-[320px] flex-1 items-center gap-1.5 rounded-md border border-input bg-field px-2">
          <Search className="size-3 shrink-0 text-dim" />
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Filter rows"
            className="min-w-0 flex-1 bg-transparent font-mono text-xs text-body outline-none placeholder:text-dim"
          />
        </label>
        <span className="flex-1" />
        <button
          type="button"
          className={TOOL_BTN}
          onClick={() => newQueryTab(connectionId, database, ddl(tab.schema, table))}
        >
          DDL
        </button>
        <IconButton
          label="Open query console"
          side="bottom"
          onClick={() => newQueryTab(connectionId, database, select)}
        >
          <FileCode className="size-3.5" />
        </IconButton>
      </Toolbar>
      <DataGrid
        key={`${nonce}`}
        columns={columns}
        rows={shown}
        pk={table.columns.filter((c) => c.pk).map((c) => c.name)}
      />
    </div>
  )
}

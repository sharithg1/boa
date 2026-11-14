// Editor area for the selected database (tabs live in the TitleBar), laid
// out like DataGrip: a query console is a toolbar (run / stop, transaction
// mode, history, schema selector) above a SQL editor with a line-number
// gutter; its results go to the bottom Database Output panel. A table tab is
// the table's data editor: a toolbar (refresh, filter, DDL, console) above
// the data grid. Execution is simulated (state/mockData.ts).
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
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
  runQuery,
  setQuerySql,
  useStore,
  type QueryTab,
  type TableTab
} from '@/state/store'
import { usePrefs } from '@/state/prefs'
import { openHistory } from '@/state/history'
import { rangePath, statementsAt, type Range, type TextMetrics } from '@/lib/statements'
import { backend, toBackendError, type QueryResult } from '@/lib/backend'
import { Pager } from './Pager'
import { ExportMenu } from './CsvExport'
import { tableOwner, tableSource } from '@/state/csvExport'
import type { Table } from '@/state/types'
import { qualified, quoteIdent, type Catalog } from '@/lib/sql'
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
import { cachedEditPage, useTableEdits } from './TableEdits'
import { IconButton } from './IconButton'
import { Spinner } from '@/components/ui/spinner'
import { SqlHighlight, type EditorMark } from './SqlHighlight'
import { CompletionPopup, caretBox, charWidth } from './CompletionPopup'
import { StatementChooser } from './StatementChooser'
import { complete, type CompletionResult } from '@/lib/complete'

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

/** Shared by the textarea and its highlight layer so glyphs line up exactly. */
const EDITOR_TEXT = 'py-2.5 pr-3 pl-3 font-mono text-sm whitespace-pre'
/** Editor top padding (`py-2.5`) and line height (`text-sm`), for the caret-line band. */
const EDITOR_PAD = 10
const EDITOR_LINE = 20

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
  const layer = useRef<HTMLPreElement>(null)
  const box = useRef<HTMLDivElement>(null)
  // Open completion list; `explicit` once invoked with Ctrl+Space, so it
  // keeps showing as the prefix is erased.
  const [comp, setComp] = useState<{
    res: CompletionResult
    sel: number
    explicit: boolean
  } | null>(null)
  // The statement at the caret is outlined; ⌘↵ arms it (filled) and Enter
  // runs it. With subqueries at the caret it is a chooser (statement vs.
  // subqueries). Cleared when the caret moves, the text changes or the
  // editor loses focus. `caret`: the caret while the editor has focus.
  const [chooser, setChooser] = useState<{ items: Range[]; sel: number; at: number } | null>(null)
  const [caret, setCaret] = useState<number | null>(null)
  // Caret line (0-based) for the current-line band; null with a selection.
  const [caretLine, setCaretLine] = useState<number | null>(0)
  const [scrollTop, setScrollTop] = useState(0)
  /** Set while a completion is inserted, so that edit does not reopen the list. */
  const accepting = useRef(false)
  // Names to colour as schema / table / column references.
  const catalog = useMemo<Catalog | undefined>(() => {
    if (!db) return undefined
    const lower = (xs: string[]) => new Set(xs.map((x) => x.toLowerCase()))
    const tables = db.schemas.flatMap((s) => s.tables)
    return {
      schemas: lower(db.schemas.map((s) => s.name)),
      tables: lower(tables.map((t) => t.name)),
      columns: lower(tables.flatMap((t) => t.columns.map((c) => c.name)))
    }
  }, [db])
  const lines = tab.sql.split('\n').length

  const exec = (sql: string) => runQuery(connectionId, database, tab.id, tab.title, sql)
  /**
   * Runs the selection. Otherwise arms the statement under the caret
   * (offering its subqueries too, when inside one); Enter, or calling this
   * again, runs the armed one. `direct` (the toolbar button)
   * skips the highlight step when there is nothing to choose between.
   */
  const run = (direct = false) => {
    const el = area.current
    if (!el) return exec(tab.sql)
    if (chooser) return pickStatement(chooser.sel)
    const selected = tab.sql.slice(el.selectionStart, el.selectionEnd)
    if (selected.trim()) return exec(selected)
    const items = onStatement(tab.sql, el.selectionStart)
    if (items.length === 0) return
    if (direct && items.length === 1) return exec(tab.sql.slice(items[0]!.from, items[0]!.to))
    setComp(null)
    el.focus() // from the toolbar button: keys must reach the editor
    setChooser({ items, sel: 0, at: el.selectionStart })
  }
  const pickStatement = (i: number) => {
    const r = chooser?.items[i]
    setChooser(null)
    if (r) exec(tab.sql.slice(r.from, r.to))
  }
  const metrics = (): TextMetrics | null => {
    const el = area.current
    if (!el) return null
    const cs = getComputedStyle(el)
    return {
      charWidth: charWidth(cs),
      lineHeight: Number.parseFloat(cs.lineHeight) || 20,
      padLeft: Number.parseFloat(cs.paddingLeft) || 0,
      padTop: Number.parseFloat(cs.paddingTop) || 0,
      tabSize: Number.parseInt(cs.tabSize, 10) || 8
    }
  }
  // Fill the armed statement, else outline the one the caret is on.
  // Cheap (one pass over the script), so recomputed each render.
  const marks = ((): EditorMark[] => {
    const armed = chooser?.items[chooser.sel]
    const r = armed ?? (caret == null ? undefined : onStatement(tab.sql, caret)[0])
    const m = r && metrics()
    return r && m ? [{ d: rangePath(tab.sql, r, m), kind: armed ? 'fill' : 'outline' }] : []
  })()
  const suggest = (sql: string, pos: number, explicit: boolean) => {
    const res = db
      ? complete(sql, pos, db.schemas, { defaultSchema: schema, searchPath: ['public'], explicit })
      : null
    setComp(res ? { res, sel: 0, explicit } : null)
  }
  /** Replaces the word at the caret (to its end with `whole`) with item `i`. */
  const accept = (i: number, whole: boolean) => {
    const el = area.current
    const item = comp?.res.items[i]
    if (!el || !comp || !item) return
    const from = comp.res.from
    const to = whole ? comp.res.to : el.selectionStart
    const caret = from + (item.caret ?? item.insert.length)
    setComp(null)
    el.focus()
    el.setSelectionRange(from, to)
    // insertText keeps the edit on the textarea's own undo stack.
    accepting.current = true
    const ok = document.execCommand('insertText', false, item.insert)
    accepting.current = false
    if (ok) {
      el.setSelectionRange(caret, caret)
      // A schema was picked (`billing.`): go straight on to its tables.
      if (item.insert.endsWith('.')) suggest(el.value, caret, false)
    } else {
      setQuerySql(key, tab.id, tab.sql.slice(0, from) + item.insert + tab.sql.slice(to))
      requestAnimationFrame(() => el.setSelectionRange(caret, caret))
    }
  }
  const trackCaret = () => {
    const el = area.current
    if (!el) return
    setCaret(el.selectionStart === el.selectionEnd ? el.selectionStart : null)
    // The DOM value: during onChange `tab.sql` is still the previous text.
    const before = el.value.slice(0, el.selectionStart).split('\n')
    setCaretLine(el.selectionStart === el.selectionEnd ? before.length - 1 : null)
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
          onClick={() => run(true)}
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
        <IconButton label="Query history (⌘⌥E)" side="bottom" onClick={openHistory}>
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
      <div className="relative flex min-h-0 flex-1 bg-editor">
        {caretLine != null && (
          // DataGrip's caret-row band, across the gutter and the text.
          <div
            aria-hidden
            className="pointer-events-none absolute inset-x-0 h-5 bg-editor-line"
            style={{ top: EDITOR_PAD + caretLine * EDITOR_LINE - scrollTop }}
          />
        )}
        <div
          ref={gutter}
          aria-hidden
          className="no-scrollbar relative w-14 shrink-0 overflow-hidden border-r border-editor-divider py-2.5 pr-5 text-right font-mono text-sm text-editor-gutter select-none"
        >
          {Array.from({ length: lines }, (_, i) => (
            <div key={i} className={cn(i === caretLine && 'text-editor-gutter-active')}>
              {i + 1}
            </div>
          ))}
        </div>
        <div ref={box} className="relative min-w-0 flex-1 overflow-hidden">
          <SqlHighlight
            ref={layer}
            sql={tab.sql}
            catalog={catalog}
            marks={marks}
            className={EDITOR_TEXT}
          />
          <textarea
            ref={area}
            autoFocus
            value={tab.sql}
            spellCheck={false}
            wrap="off"
            autoCapitalize="off"
            autoCorrect="off"
            aria-label="SQL"
            onChange={(e) => {
              const { value, selectionStart: pos } = e.target
              setQuerySql(key, tab.id, value)
              setChooser(null)
              trackCaret()
              if (accepting.current) return
              // Open on typing a name character or `.`; follow edits while open.
              const input = e.nativeEvent as InputEvent
              const typed = input.inputType === 'insertText' && /[\w$."]$/.test(input.data ?? '')
              const erased = !!comp && input.inputType?.startsWith('delete')
              if (typed || erased) suggest(value, pos, comp?.explicit ?? false)
              else setComp(null)
            }}
            onSelect={(e) => {
              trackCaret()
              setChooser((c) => (c && e.currentTarget.selectionStart !== c.at ? null : c))
              // Caret moved off the word being completed: close.
              const { selectionStart: a, selectionEnd: b } = e.currentTarget
              setComp((c) => (c && (a !== b || a < c.res.from || a > c.res.to) ? null : c))
            }}
            onFocus={trackCaret}
            onBlur={() => {
              useStore.setState({ cursor: null })
              setComp(null)
              setChooser(null)
              setCaret(null)
            }}
            onScroll={(e) => {
              setComp((c) => c && { ...c })
              setChooser((c) => c && { ...c })
              const { scrollTop, scrollLeft } = e.currentTarget
              setScrollTop(scrollTop)
              if (gutter.current) gutter.current.scrollTop = scrollTop
              if (layer.current) {
                layer.current.scrollTop = scrollTop
                layer.current.scrollLeft = scrollLeft
              }
            }}
            onKeyDown={(e) => {
              if (e.ctrlKey && (e.key === ' ' || e.code === 'Space')) {
                e.preventDefault()
                suggest(tab.sql, e.currentTarget.selectionStart, true)
                return
              }
              if (chooser && !e.altKey) {
                const n = chooser.items.length
                if (n > 1 && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) {
                  e.preventDefault()
                  const d = e.key === 'ArrowDown' ? 1 : -1
                  return setChooser({ ...chooser, sel: (chooser.sel + d + n) % n })
                }
                if (e.key === 'Enter') {
                  // Enter (or ⌘↵ again) runs the armed statement.
                  e.preventDefault()
                  return pickStatement(chooser.sel)
                }
                if (e.key === 'Escape') {
                  e.preventDefault()
                  return setChooser(null)
                }
                if (!['Shift', 'Meta', 'Control'].includes(e.key)) setChooser(null)
              }
              if (comp && !e.metaKey && !e.ctrlKey && !e.altKey) {
                const n = comp.res.items.length
                const move = (d: number) => {
                  e.preventDefault()
                  setComp({ ...comp, sel: (comp.sel + d + n) % n })
                }
                if (e.key === 'ArrowDown') return move(1)
                if (e.key === 'ArrowUp') return move(-1)
                if (e.key === 'PageDown') return move(Math.min(9, n - 1 - comp.sel) || 0)
                if (e.key === 'PageUp') return move(-Math.min(9, comp.sel) || 0)
                if (e.key === 'Escape') {
                  e.preventDefault()
                  return setComp(null)
                }
                if ((e.key === 'Enter' || e.key === 'Tab') && !e.shiftKey) {
                  // Enter inserts before the rest of the word, Tab replaces it.
                  e.preventDefault()
                  return accept(comp.sel, e.key === 'Tab')
                }
              }
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
            // Text is drawn by the highlight layer underneath; the textarea
            // only shows the caret and the selection.
            className={cn(
              EDITOR_TEXT,
              'relative h-full w-full resize-none bg-transparent text-transparent caret-body outline-none selection:bg-status-run/35'
            )}
          />
          {chooser && chooser.items.length > 1 && area.current && box.current && (
            <StatementChooser
              sql={tab.sql}
              items={chooser.items}
              selected={chooser.sel}
              caret={caretBox(area.current, chooser.at)}
              bounds={{ width: box.current.clientWidth, height: box.current.clientHeight }}
              onPick={pickStatement}
              onHover={(i) => setChooser({ ...chooser, sel: i })}
            />
          )}
          {comp && area.current && box.current && (
            <CompletionPopup
              items={comp.res.items}
              selected={comp.sel}
              caret={caretBox(area.current, comp.res.from)}
              bounds={{ width: box.current.clientWidth, height: box.current.clientHeight }}
              onPick={(i) => accept(i, false)}
              onHover={(i) => setComp({ ...comp, sel: i })}
            />
          )}
        </div>
      </div>
    </div>
  )
}

function ddl(schema: string, t: Table): string {
  const cols = t.columns.map(
    (c) =>
      `    ${quoteIdent(c.name)} ${c.type}${c.nullable ? '' : ' not null'}${c.default ? ` default ${c.default}` : ''}`
  )
  const pk = t.columns.filter((c) => c.pk).map((c) => quoteIdent(c.name))
  if (pk.length) cols.push(`    primary key (${pk.join(', ')})`)
  return t.kind === 'view'
    ? `create view ${qualified(schema, t.name)} as\n    select …;\n`
    : `create table ${qualified(schema, t.name)}\n(\n${cols.join(',\n')}\n);\n`
}

type Load =
  | { state: 'loading' }
  | { state: 'ok'; result: QueryResult }
  | { state: 'err'; message: string }

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
  const connected = useStore((s) => s.connections[connectionId]?.state === 'connected')
  const rowLimit = usePrefs((s) => s.rowLimit)
  const [nonce, setNonce] = useState(0)
  // Pending grid edits keep their page across tab switches (no reload under them).
  const [page, setPage] = useState(() => {
    const c = cachedEditPage(tab.id)
    return c
      ? {
          offset: c.offset,
          rowLimit: c.rowLimit,
          table: `select * from ${qualified(tab.schema, tab.table)}`
        }
      : { offset: 0, rowLimit, table: '' }
  })
  const [paging, setPaging] = useState(false)
  const [filter, setFilter] = useState('')
  const [load, setLoad] = useState<Load>(() => {
    const c = cachedEditPage(tab.id)
    return c ? { state: 'ok', result: c.result } : { state: 'loading' }
  })
  const table = findTable(connectionId, database, tab.schema, tab.table)
  const select = `select * from ${qualified(tab.schema, tab.table)}`
  // A new page size (or table) starts over from the first page.
  const offset = page.rowLimit === rowLimit && page.table === select ? page.offset : 0
  const setOffset = (o: number) => setPage({ offset: o, rowLimit, table: select })

  // biome-ignore lint/correctness/useExhaustiveDependencies: nonce forces a reload
  useEffect(() => {
    if (!connected) return
    const cached = cachedEditPage(tab.id)
    if (cached && cached.offset === offset && cached.rowLimit === rowLimit) return
    let stale = false
    setLoad((l) => (l.state === 'ok' ? l : { state: 'loading' }))
    setPaging(true)
    backend
      .runQuery(connectionId, database, select, rowLimit, offset)
      .then((result) => !stale && setLoad({ state: 'ok', result }))
      .catch((e) => !stale && setLoad({ state: 'err', message: toBackendError(e).message }))
      .finally(() => !stale && setPaging(false))
    return () => {
      stale = true
    }
  }, [connectionId, database, select, rowLimit, offset, connected, nonce])

  const edits = useTableEdits({
    tabId: tab.id,
    connectionId,
    database,
    schema: tab.schema,
    table,
    result: load.state === 'ok' ? load.result : null,
    offset,
    rowLimit,
    filter,
    onSubmitted: () => setNonce((n) => n + 1)
  })

  return (
    <div className="flex h-full flex-col overflow-hidden">
      <Toolbar>
        <IconButton
          label="Reload"
          side="bottom"
          disabled={!connected || paging}
          onClick={() => edits.guard('reload', () => setNonce((n) => n + 1))}
        >
          <RefreshCw className="size-3.5" />
        </IconButton>
        {load.state === 'ok' && (
          <Pager
            offset={offset}
            count={load.result.rows.length}
            pageSize={rowLimit}
            more={load.result.truncated}
            busy={paging}
            onPage={(o) => edits.guard('change the page', () => setOffset(o))}
          />
        )}
        <span className="px-1.5 text-xs text-dim">
          {load.state === 'ok'
            ? `${offset === 0 && !load.result.truncated ? `${load.result.rows.length.toLocaleString()} rows · ` : ''}${load.result.ms} ms`
            : load.state === 'loading' && connected
              ? 'Loading…'
              : ''}
          {table && table.rows > 0 ? ` · ~${table.rows.toLocaleString()} total` : ''}
        </span>
        {edits.toolbar}
        <Sep />
        <label className="flex h-6 min-w-0 max-w-[320px] flex-1 items-center gap-1.5 rounded-md border border-input bg-field px-2">
          <Search className="size-3 shrink-0 text-dim" />
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Filter loaded rows"
            className="min-w-0 flex-1 bg-transparent font-mono text-xs text-body outline-none placeholder:text-dim"
          />
        </label>
        <span className="flex-1" />
        <ExportMenu
          source={{
            ...tableSource(connectionId, database, tab.schema, tab.table),
            columns: load.state === 'ok' ? load.result.columns.map((c) => c.name) : undefined,
            rows: edits.rows
          }}
          owner={tableOwner(connectionId, database, tab.schema, tab.table)}
          disabled={!connected}
        />
        {table && (
          <button
            type="button"
            className={TOOL_BTN}
            onClick={() => newQueryTab(connectionId, database, ddl(tab.schema, table))}
          >
            DDL
          </button>
        )}
        <IconButton
          label="Open query console"
          side="bottom"
          onClick={() => newQueryTab(connectionId, database, `${select};`)}
        >
          <FileCode className="size-3.5" />
        </IconButton>
      </Toolbar>
      {!connected ? (
        <div className="grid flex-1 place-items-center text-sm text-quiet">Not connected</div>
      ) : load.state === 'err' ? (
        <div className="grid flex-1 place-items-center px-6 text-center text-sm text-status-err-text select-text">
          {load.message}
        </div>
      ) : load.state === 'loading' ? (
        <div className="grid flex-1 place-items-center text-sm text-quiet">
          <Spinner className="size-4" />
        </div>
      ) : (
        <DataGrid
          key={`${nonce}-${offset}`}
          columns={load.result.columns}
          rows={edits.rows}
          firstRow={filter.trim() ? 1 : offset + 1}
          pk={table?.columns.filter((c) => c.pk).map((c) => c.name)}
          editing={edits.editing}
        />
      )}
      {edits.dialogs}
    </div>
  )
}

/**
 * The statement at `pos` (then its subqueries there), or [] when the caret
 * is not on one: inside it, or on its first / last line around it, but not
 * on a blank line between statements.
 */
function onStatement(sql: string, pos: number): Range[] {
  const items = statementsAt(sql, pos)
  const r = items[0]
  if (!r) return []
  const gap = pos < r.from ? sql.slice(pos, r.from) : pos > r.to ? sql.slice(r.to, pos) : ''
  return gap.includes('\n') ? [] : items
}

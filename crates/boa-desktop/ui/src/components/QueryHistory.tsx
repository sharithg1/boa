// Query History (DataGrip's), opened with ⌘⌥E, the console toolbar's
// history button or from the ⌘K palette: every statement run on the active
// connection (or on all of them), newest first, searchable as you type, with
// an errors-only filter. The list is windowed so thousands of entries stay
// fast. The detail pane shows the whole statement and error; ⏎ inserts the
// statement at the caret of the current console, ⌘⏎ opens it in a new
// console on its database, ⌘C copies it, ⌘⌫ deletes the entry.
import { useDeferredValue, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import {
  CircleAlert,
  Copy,
  History,
  Search,
  SquarePlus,
  TextCursorInput,
  Trash2
} from 'lucide-react'
import { dbKey, newQueryTab, pushToast, setQuerySql, useStore } from '@/state/store'
import { clearHistory, deleteHistoryEntries, loadHistory, useQueryHistory } from '@/state/history'
import { usePrefs } from '@/state/prefs'
import {
  duration,
  filterHistory,
  insertionText,
  mergeNewestFirst,
  outcomeLabel,
  sqlPreview,
  type HistoryItem
} from '@/lib/history'
import { ago } from '@/lib/format'
import { cn } from '@/lib/utils'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle
} from '@/components/ui/alert-dialog'
import { SqlText } from './SqlHighlight'

const isMac = () => navigator.platform.toLowerCase().includes('mac')

/** The active editor tab, when it is a console. */
function activeConsole(s = useStore.getState()) {
  const { active, workspaces } = s
  if (!active) return null
  const key = dbKey(active.connectionId, active.database)
  const ws = workspaces[key]
  const tab = ws?.tabs.find((t) => t.id === ws.activeTab)
  return tab?.kind === 'query' ? { key, tab } : null
}

/**
 * Inserts `sql` at the caret (replacing the selection) of the current
 * console. Through the textarea's own editing, so ⌘Z undoes it.
 */
function insertIntoConsole(sql: string): boolean {
  const target = activeConsole()
  if (!target) return false
  const { key, tab } = target
  const el = document.querySelector<HTMLTextAreaElement>('main textarea[aria-label="SQL"]')
  if (!el) {
    const sep = tab.sql && !tab.sql.endsWith('\n') ? '\n' : ''
    setQuerySql(key, tab.id, tab.sql + sep + sql)
    return true
  }
  el.focus() // keeps the caret it had before the history opened
  const { selectionStart: a, selectionEnd: b, value } = el
  const text = insertionText(value.slice(0, a), value.slice(b), sql)
  if (!document.execCommand('insertText', false, text))
    setQuerySql(key, tab.id, value.slice(0, a) + text + value.slice(b))
  return true
}

function copySql(sql: string) {
  void navigator.clipboard?.writeText(sql).catch(() => {})
  pushToast({ kind: 'info', title: 'Statement copied' }, 2400)
}

export function QueryHistory() {
  const open = useQueryHistory((s) => s.open)
  // Inserting waits until the dialog has closed and handed focus back.
  const pendingInsert = useRef<string | null>(null)
  const close = () => useQueryHistory.setState({ open: false })
  return (
    <Dialog open={open} onOpenChange={(o) => useQueryHistory.setState({ open: o })}>
      <DialogContent
        showCloseButton={false}
        overlayClassName="bg-black/50"
        onCloseAutoFocus={(e) => {
          const sql = pendingInsert.current
          pendingInsert.current = null
          if (sql != null) {
            e.preventDefault()
            insertIntoConsole(sql)
          }
        }}
        className="top-[8vh] flex h-[min(680px,84vh)] w-[1000px] max-w-[calc(100vw-32px)] translate-y-0 flex-col gap-0 overflow-hidden rounded-xl p-0 shadow-palette sm:max-w-[1000px]"
      >
        <DialogTitle className="sr-only">Query history</DialogTitle>
        <DialogDescription className="sr-only">
          Statements run from the client. Type to search their SQL.
        </DialogDescription>
        {open && (
          <HistoryBody
            onInsert={(sql) => {
              pendingInsert.current = sql
              close()
            }}
            onClose={close}
          />
        )}
      </DialogContent>
    </Dialog>
  )
}

const ROW_H = 64
const OVERSCAN = 6

type Scope = 'connection' | 'all'

function HistoryBody({
  onInsert,
  onClose
}: {
  onInsert: (sql: string) => void
  onClose: () => void
}) {
  const active = useStore((s) => s.active)
  const connections = useStore((s) => s.connections)
  const order = useStore((s) => s.connectionOrder)
  const hasConsole = useStore((s) => !!activeConsole(s))
  const recording = usePrefs((s) => s.recordHistory)
  const lists = useQueryHistory((s) => s.lists)
  const loading = useQueryHistory((s) => s.loading)
  const [scope, setScope] = useState<Scope>(active ? 'connection' : 'all')
  const [q, setQ] = useState('')
  const [errorsOnly, setErrorsOnly] = useState(false)
  const [sel, setSel] = useState(0)
  const [confirmClear, setConfirmClear] = useState(false)
  const query = useDeferredValue(q)

  const ids = useMemo(
    () => (scope === 'connection' && active ? [active.connectionId] : order),
    [scope, active, order]
  )
  useEffect(() => {
    for (const id of ids) void loadHistory(id)
  }, [ids])

  const items = useMemo(
    () =>
      mergeNewestFirst(
        ids.map((id) => (lists[id] ?? []).map((r): HistoryItem => ({ ...r, connectionId: id })))
      ),
    [ids, lists]
  )
  const shown = useMemo(
    () => filterHistory(items, { query, errorsOnly }),
    [items, query, errorsOnly]
  )
  const busy = ids.some((id) => loading[id])
  const idx = Math.min(sel, Math.max(0, shown.length - 1))
  const current = shown[idx]

  // Windowed list: only the rows in view (plus a few) are rendered.
  const listRef = useRef<HTMLDivElement>(null)
  const [scrollTop, setScrollTop] = useState(0)
  const [viewH, setViewH] = useState(480)
  useLayoutEffect(() => {
    const el = listRef.current
    if (!el) return
    const ro = new ResizeObserver(() => setViewH(el.clientHeight))
    ro.observe(el)
    setViewH(el.clientHeight)
    return () => ro.disconnect()
  }, [])
  useEffect(() => {
    const el = listRef.current
    if (!el) return
    const top = idx * ROW_H
    if (top < el.scrollTop) el.scrollTop = top
    else if (top + ROW_H > el.scrollTop + el.clientHeight)
      el.scrollTop = top + ROW_H - el.clientHeight
  }, [idx])
  const first = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN)
  const last = Math.min(shown.length, Math.ceil((scrollTop + viewH) / ROW_H) + OVERSCAN)

  const move = (d: number) => {
    if (!shown.length) return
    setSel(Math.max(0, Math.min(shown.length - 1, idx + d)))
  }
  const insert = (r: HistoryItem | undefined) => {
    if (!r) return
    if (hasConsole) onInsert(r.sql)
    else openInConsole(r)
  }
  const openInConsole = (r: HistoryItem | undefined) => {
    if (!r) return
    onClose()
    newQueryTab(r.connectionId, r.database, r.sql)
  }
  const remove = (r: HistoryItem | undefined) => {
    if (r) void deleteHistoryEntries(r.connectionId, [r.id])
  }
  const resetSel = () => {
    setSel(0)
    if (listRef.current) listRef.current.scrollTop = 0
  }
  const pickScope = (next: Scope) => {
    setScope(next)
    resetSel()
  }

  const connName = (id: string) => connections[id]?.name ?? id
  const total = items.length
  const scopeLabel =
    scope === 'connection' && active ? connName(active.connectionId) : 'all connections'

  return (
    <>
      <div className="flex h-[46px] shrink-0 items-center gap-2.5 border-b border-border px-3.5">
        <Search className="size-3.5 shrink-0 text-quiet" />
        <input
          autoFocus
          value={q}
          spellCheck={false}
          aria-label="Search query history"
          placeholder="Search history…  (words in the SQL, any order)"
          onChange={(e) => {
            setQ(e.target.value)
            resetSel()
          }}
          onKeyDown={(e) => {
            const mod = isMac() ? e.metaKey : e.ctrlKey
            if (e.key === 'ArrowDown') {
              e.preventDefault()
              move(1)
            } else if (e.key === 'ArrowUp') {
              e.preventDefault()
              move(-1)
            } else if (e.key === 'PageDown') {
              e.preventDefault()
              move(Math.max(1, Math.floor(viewH / ROW_H) - 1))
            } else if (e.key === 'PageUp') {
              e.preventDefault()
              move(-Math.max(1, Math.floor(viewH / ROW_H) - 1))
            } else if (e.key === 'Enter') {
              e.preventDefault()
              if (mod) openInConsole(current)
              else insert(current)
            } else if (mod && e.code === 'KeyC') {
              const t = e.currentTarget
              if (t.selectionStart !== t.selectionEnd || !current) return
              e.preventDefault()
              copySql(current.sql)
            } else if (mod && e.key === 'Backspace') {
              e.preventDefault()
              remove(current)
            }
          }}
          className="min-w-0 flex-1 border-none bg-transparent text-base text-foreground outline-none placeholder:text-faint"
        />
        <div className="flex shrink-0 items-center gap-0.5" role="group" aria-label="Scope">
          {active && (
            <Pill selected={scope === 'connection'} onClick={() => pickScope('connection')}>
              {connName(active.connectionId)}
            </Pill>
          )}
          <Pill selected={scope === 'all'} onClick={() => pickScope('all')}>
            All connections
          </Pill>
          <span aria-hidden className="mx-1 h-4 w-px bg-border" />
          <Pill
            selected={errorsOnly}
            aria-pressed={errorsOnly}
            onClick={() => {
              setErrorsOnly((v) => !v)
              resetSel()
            }}
          >
            <CircleAlert className="size-3" />
            Errors only
          </Pill>
        </div>
        <span className="shrink-0 font-mono text-2xs text-faint">
          {shown.length === total
            ? total.toLocaleString()
            : `${shown.length.toLocaleString()} of ${total.toLocaleString()}`}
        </span>
      </div>
      <div className="flex min-h-0 flex-1">
        <div
          ref={listRef}
          role="listbox"
          aria-label="Statements"
          onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
          className="relative min-h-0 min-w-0 flex-[3] overflow-y-auto"
        >
          {shown.length > 0 ? (
            <div style={{ height: shown.length * ROW_H }} className="relative">
              {shown.slice(first, last).map((r, k) => {
                const i = first + k
                return (
                  <HistoryRow
                    key={`${r.connectionId}:${r.id}`}
                    r={r}
                    top={i * ROW_H}
                    selected={i === idx}
                    connection={scope === 'all' ? connName(r.connectionId) : null}
                    onSelect={() => setSel(i)}
                    onOpen={() => insert(r)}
                  />
                )
              })}
            </div>
          ) : (
            <div className="grid h-full place-items-center px-8 text-center text-sm text-quiet">
              {busy
                ? 'Loading…'
                : total === 0
                  ? recording
                    ? 'No statements yet. Statements you run are saved here.'
                    : 'No statements saved. Recording is off (Settings → Queries).'
                  : 'No statements match.'}
            </div>
          )}
        </div>
        <div className="flex min-h-0 min-w-0 flex-[2] flex-col border-l border-border">
          {current ? (
            <Detail
              r={current}
              connection={connName(current.connectionId)}
              hasConsole={hasConsole}
              onInsert={() => insert(current)}
              onOpen={() => openInConsole(current)}
              onDelete={() => remove(current)}
            />
          ) : (
            <div className="grid flex-1 place-items-center text-sm text-quiet">
              <History className="size-5 text-dim" />
            </div>
          )}
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-4 border-t border-border px-3.5 py-2 font-mono text-2xs text-faint">
        <span>↑↓ move</span>
        <span>⏎ {hasConsole ? 'insert at caret' : 'open in console'}</span>
        <span>⌘⏎ new console</span>
        <span>⌘C copy</span>
        <span>⌘⌫ delete</span>
        <span className="flex-1" />
        {!recording && <span className="font-sans text-status-wait">Recording is off</span>}
        <button
          type="button"
          disabled={total === 0}
          onClick={() => setConfirmClear(true)}
          className="cursor-pointer font-sans text-xs text-quiet hover:text-foreground disabled:cursor-default disabled:opacity-40"
        >
          Clear history…
        </button>
        <span>esc</span>
      </div>
      <AlertDialog open={confirmClear} onOpenChange={setConfirmClear}>
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle>Clear query history?</AlertDialogTitle>
            <AlertDialogDescription>
              Deletes the {total.toLocaleString()} saved statement{total === 1 ? '' : 's'} of{' '}
              {scopeLabel}. This cannot be undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                for (const id of ids) void clearHistory(id)
                resetSel()
              }}
            >
              Clear
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}

function Pill({
  selected,
  className,
  ...props
}: React.ComponentProps<'button'> & { selected: boolean }) {
  return (
    <button
      type="button"
      {...props}
      className={cn(
        'flex h-6 max-w-40 cursor-pointer items-center gap-1 truncate rounded-md px-2 text-xs',
        selected ? 'bg-card text-foreground' : 'text-quiet hover:bg-hover hover:text-soft',
        className
      )}
    />
  )
}

function HistoryRow({
  r,
  top,
  selected,
  connection,
  onSelect,
  onOpen
}: {
  r: HistoryItem
  top: number
  selected: boolean
  connection: string | null
  onSelect: () => void
  onOpen: () => void
}) {
  const pv = sqlPreview(r.sql)
  return (
    <div
      role="option"
      aria-selected={selected}
      onClick={onSelect}
      onDoubleClick={onOpen}
      style={{ top, height: ROW_H }}
      className={cn(
        'absolute inset-x-1 flex cursor-default gap-2 rounded-md px-2 py-1.5',
        selected ? 'bg-accent' : 'hover:bg-hover'
      )}
    >
      <span className="mt-0.5 grid size-3.5 shrink-0 place-items-center">
        {r.error && <CircleAlert className="size-3.5 text-status-err-text" aria-label="Failed" />}
      </span>
      <div className="min-w-0 flex-1">
        <pre className="m-0 h-9 overflow-hidden font-mono text-xs leading-[18px] whitespace-pre text-body">
          <SqlText sql={pv.text} />
          {pv.more && <span className="text-faint"> …</span>}
        </pre>
        <div className="flex min-w-0 items-center gap-1 truncate text-2xs leading-4 text-faint">
          {connection && <span className="shrink-0 text-quiet">{connection}</span>}
          {connection && <span className="text-dim">·</span>}
          <span className="truncate font-mono">{r.database}</span>
          <span className="text-dim">·</span>
          <span className="shrink-0" title={new Date(r.at).toLocaleString()}>
            {ago(r.at)}
          </span>
          <span className="text-dim">·</span>
          <span className="shrink-0">{duration(r.ms)}</span>
          <span className="text-dim">·</span>
          <span className={cn('shrink-0', r.error && 'text-status-err-text')}>
            {outcomeLabel(r)}
          </span>
        </div>
      </div>
    </div>
  )
}

function Detail({
  r,
  connection,
  hasConsole,
  onInsert,
  onOpen,
  onDelete
}: {
  r: HistoryItem
  connection: string
  hasConsole: boolean
  onInsert: () => void
  onOpen: () => void
  onDelete: () => void
}) {
  return (
    <>
      <div className="flex shrink-0 flex-col gap-1 border-b border-border px-3 py-2.5">
        <div className="flex min-w-0 items-center gap-1.5 text-sm">
          <span className="truncate text-soft">{connection}</span>
          <span className="text-dim">/</span>
          <span className="truncate font-mono text-soft">{r.database}</span>
          {r.source && <span className="truncate text-xs text-dim">· {r.source}</span>}
        </div>
        <div className="text-xs text-quiet">
          {new Date(r.at).toLocaleString()} · {duration(r.ms)} ·{' '}
          <span className={cn(r.error && 'text-status-err-text')}>{outcomeLabel(r)}</span>
        </div>
        <div className="mt-1 flex flex-wrap items-center gap-1">
          {hasConsole && (
            <Button size="xs" variant="outline" onClick={onInsert} title="Insert at caret (⏎)">
              <TextCursorInput />
              Insert
            </Button>
          )}
          <Button size="xs" variant="outline" onClick={onOpen} title="Open in new console (⌘⏎)">
            <SquarePlus />
            New console
          </Button>
          <Button size="xs" variant="outline" onClick={() => copySql(r.sql)} title="Copy (⌘C)">
            <Copy />
            Copy
          </Button>
          <span className="flex-1" />
          <Button
            size="xs"
            variant="ghost"
            onClick={onDelete}
            title="Delete entry (⌘⌫)"
            className="text-quiet"
          >
            <Trash2 />
            Delete
          </Button>
        </div>
      </div>
      {r.error && (
        <div className="max-h-[40%] shrink-0 overflow-auto border-b border-border bg-status-err-text/5 px-3 py-2 font-mono text-xs leading-5 whitespace-pre-wrap break-words text-status-err-text select-text">
          {r.error}
          {r.detail && <div className="text-status-err-text/80">{r.detail}</div>}
        </div>
      )}
      <pre className="m-0 min-h-0 flex-1 overflow-auto px-3 py-2.5 font-mono text-xs leading-5 whitespace-pre text-body select-text">
        <SqlText sql={r.sql} />
      </pre>
    </>
  )
}

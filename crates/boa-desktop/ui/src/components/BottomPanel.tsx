// Bottom tool window, after DataGrip's "Database Output": a header with the
// Output log and one tab per console result of the selected database, the
// selected result's source / timing on the right and a hide button; below,
// the result grid or the log. Resizable from its top edge (double-click
// resets); toggled with ⌥J or the status bar button.
import { useState } from 'react'
import { Minus, SquareTerminal, Table2, Trash2, X } from 'lucide-react'
import { closeResult, dbKey, pageResult, useStore } from '@/state/store'
import { clampBottomHeight, DEFAULT_PREFS, setPrefs, usePrefs } from '@/state/prefs'
import type { HistoryEntry } from '@/state/types'
import { cn } from '@/lib/utils'
import { resultSource } from '@/state/csvExport'
import { ExportMenu } from './CsvExport'
import { DataGrid } from './DataGrid'
import { IconButton } from './IconButton'
import { Pager } from './Pager'
import { RunBar } from './RunProgress'
import { Spinner } from '@/components/ui/spinner'
import { PanelResizeHandle } from './PanelResizeHandle'

export function BottomPanel() {
  const savedHeight = usePrefs((s) => s.bottomHeight)
  const [dragHeight, setDragHeight] = useState<number | null>(null)
  const height = dragHeight ?? clampBottomHeight(savedHeight)
  const active = useStore((s) => s.active)
  const view = useStore((s) => s.bottomView)
  const results = useStore((s) => s.results)
  const tabs = useStore((s) =>
    s.active ? s.workspaces[dbKey(s.active.connectionId, s.active.database)]?.tabs : undefined
  )
  // Result tabs follow the editor tabs of the selected database.
  const resultTabs = (tabs ?? []).filter((t) => results[t.id])
  const current = view !== 'output' ? results[view] : undefined
  const sourceTab = tabs?.find((t) => t.id === current?.tabId)
  const conn = useStore((s) => (active ? s.connections[active.connectionId] : undefined))
  const running = useStore((s) => s.running)
  const paging = current ? current.tabId in running : false
  // A statement of this database is in flight: its outcome lands here.
  const busy = (tabs ?? []).some((t) => t.id in running)

  return (
    <section
      className="relative flex shrink-0 flex-col border-t border-border bg-background"
      style={{ height }}
    >
      <PanelResizeHandle
        edge="top"
        label="Resize output panel"
        width={height}
        clamp={clampBottomHeight}
        onDrag={setDragHeight}
        onCommit={(h) => setPrefs({ bottomHeight: h })}
        onReset={() => setPrefs({ bottomHeight: DEFAULT_PREFS.bottomHeight })}
      />
      <div className="relative flex h-9 shrink-0 items-center gap-0.5 border-b border-border px-2 select-none">
        {busy && <RunBar />}
        <span className="mr-2 pl-1 text-sm font-medium text-foreground">Database Output</span>
        <div
          className="no-scrollbar flex min-w-0 items-center gap-0.5 overflow-x-auto"
          role="tablist"
        >
          <PanelTab
            selected={view === 'output' || !current}
            onClick={() => useStore.setState({ bottomView: 'output' })}
            icon={<SquareTerminal className="size-3.5 shrink-0" />}
          >
            Output
          </PanelTab>
          {resultTabs.map((t) => {
            const r = results[t.id]!
            return (
              <PanelTab
                key={t.id}
                selected={view === t.id}
                onClick={() => useStore.setState({ bottomView: t.id })}
                onClose={() => closeResult(t.id)}
                icon={
                  t.id in running ? (
                    <Spinner aria-label="Running" className="size-3.5 shrink-0 text-status-run" />
                  ) : (
                    <Table2 className="size-3.5 shrink-0" />
                  )
                }
                title={r.sql}
              >
                <span className="font-mono text-sm">{r.title}</span>
              </PanelTab>
            )
          })}
        </div>
        <span className="min-w-4 flex-1" />
        {current && (
          <span className="shrink-0 truncate px-1.5 text-xs text-dim">
            {sourceTab?.kind === 'query' ? sourceTab.title : ''}
            {conn ? ` [${conn.name}]` : ''} · {current.ms} ms
            {current.truncated && !current.pageable ? ' · row limit reached' : ''}
          </span>
        )}
        {current && <ExportMenu source={resultSource(current)} owner={current.tabId} />}
        {current?.pageable && (
          <Pager
            offset={current.offset}
            count={current.rows.length}
            pageSize={current.pageSize}
            more={!!current.truncated}
            busy={paging}
            onPage={(o) => void pageResult(current.tabId, o)}
          />
        )}
        {view === 'output' && active && (
          <IconButton
            label="Clear output"
            side="top"
            onClick={() =>
              useStore.setState((s) => ({ history: { ...s.history, [active.connectionId]: [] } }))
            }
          >
            <Trash2 className="size-3.5" />
          </IconButton>
        )}
        <IconButton
          label="Hide (⌥J)"
          side="top"
          onClick={() => useStore.setState({ bottomOpen: false })}
        >
          <Minus className="size-4" />
        </IconButton>
      </div>
      {current ? (
        <DataGrid
          key={`${current.tabId}-${current.at}-${current.offset}`}
          columns={current.columns}
          rows={current.rows}
          firstRow={current.offset + 1}
        />
      ) : (
        <OutputLog connectionId={active?.connectionId} />
      )}
    </section>
  )
}

function PanelTab({
  selected,
  onClick,
  onClose,
  icon,
  title,
  children
}: {
  selected: boolean
  onClick: () => void
  onClose?: () => void
  icon: React.ReactNode
  title?: string
  children: React.ReactNode
}) {
  return (
    <div
      role="tab"
      aria-selected={selected}
      title={title}
      onClick={onClick}
      onAuxClick={(e) => {
        if (e.button === 1 && onClose) onClose()
      }}
      className={cn(
        'group/tab flex h-7 shrink-0 cursor-default items-center gap-1.5 rounded-md pl-2 text-sm whitespace-nowrap',
        onClose ? 'pr-1' : 'pr-2',
        selected ? 'bg-card text-foreground' : 'text-quiet hover:bg-hover hover:text-soft'
      )}
    >
      <span className="text-quiet">{icon}</span>
      {children}
      {onClose && (
        <button
          type="button"
          aria-label="Close result"
          onClick={(e) => {
            e.stopPropagation()
            onClose()
          }}
          className={cn(
            'grid size-[18px] cursor-pointer place-items-center rounded-[4px] text-quiet hover:bg-hover-strong hover:text-foreground',
            !selected && 'opacity-0 group-hover/tab:opacity-100'
          )}
        >
          <X className="size-3" />
        </button>
      )}
    </div>
  )
}

const time = (ms: number) =>
  new Date(ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })

function outcome(h: HistoryEntry): string {
  if (h.error) return `ERROR: ${h.error}`
  if (h.note) return h.note
  if (h.rows != null)
    return `${h.rows} row${h.rows === 1 ? '' : 's'} retrieved starting from ${(h.offset ?? 0) + 1} in ${h.ms} ms${
      h.truncated ? ' (more available)' : ''
    }`
  if (h.affected) return `${h.affected} row${h.affected === 1 ? '' : 's'} affected in ${h.ms} ms`
  return `completed in ${h.ms} ms`
}

/** Session log of the connection's statements, oldest first. */
function OutputLog({ connectionId }: { connectionId?: string }) {
  const history = useStore((s) => (connectionId ? s.history[connectionId] : undefined))
  const entries = [...(history ?? [])].reverse()
  if (!entries.length) {
    return (
      <div className="grid flex-1 place-items-center text-sm text-quiet">
        Run a statement (⌘↵) to see its output here
      </div>
    )
  }
  return (
    <div className="min-h-0 flex-1 overflow-auto px-3 py-2 font-mono text-sm leading-6 select-text">
      {entries.map((h) => (
        <div key={h.id} className="whitespace-pre-wrap break-all">
          <div className="text-soft">
            <span className="text-dim">[{time(h.at)}] </span>
            <span className="text-quiet">{h.source ?? h.database}&gt; </span>
            {h.sql}
          </div>
          <div className={h.error ? 'text-status-err-text' : 'text-dim'}>
            <span className="opacity-0">[{time(h.at)}] </span>
            {outcome(h)}
          </div>
          {h.detail && (
            <div className="text-status-err-text/80">
              <span className="opacity-0">[{time(h.at)}] </span>
              {h.detail}
            </div>
          )}
        </div>
      ))}
    </div>
  )
}

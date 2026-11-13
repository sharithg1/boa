// Right panel (Structure / Info / History), styled after Cursor's review
// pane: its own header row of icon pills with the panel toggle at the far
// right, then hairline-ruled sections. Structure lists the active table
// tab's columns; Info shows the connection and database; History lists the
// queries run on the connection (click to reopen in a new query tab).
import { useState } from 'react'
import { Columns3, Copy, History, Info, KeyRound, Table2 } from 'lucide-react'
import { connectionString, dbKey, findTable, newQueryTab, pushToast, useStore } from '@/state/store'
import {
  clampRightPanelWidth,
  DEFAULT_PREFS,
  setPrefs,
  usePrefs,
  type RightTab
} from '@/state/prefs'
import { cn } from '@/lib/utils'
import { ago, bytes, ENGINE_LABEL } from '@/lib/format'
import { IconButton } from './IconButton'
import { PanelResizeHandle } from './PanelResizeHandle'
import { StatusDot, connDot, DOT_LABEL } from './StatusDot'
import { RightPanelToggle } from './TitleBar'

const TABS: [RightTab, string, typeof Info][] = [
  ['structure', 'Structure', Columns3],
  ['info', 'Info', Info],
  ['history', 'History', History]
]

export function RightPanel() {
  const active = useStore((s) => s.active)
  const tab = usePrefs((s) => s.rightTab)
  const savedWidth = usePrefs((s) => s.rightPanelWidth)
  // While dragging the width lives here; it is saved once, on release.
  const [dragWidth, setDragWidth] = useState<number | null>(null)
  const width = dragWidth ?? clampRightPanelWidth(savedWidth)
  return (
    <aside
      className="relative flex min-h-0 shrink-0 flex-col border-l border-border bg-background"
      style={{ width }}
    >
      <PanelResizeHandle
        edge="left"
        label="Resize right panel"
        width={width}
        clamp={clampRightPanelWidth}
        onDrag={setDragWidth}
        onCommit={(w) => setPrefs({ rightPanelWidth: w })}
        onReset={() => setPrefs({ rightPanelWidth: DEFAULT_PREFS.rightPanelWidth })}
      />
      <div
        data-tauri-drag-region="deep"
        className="flex h-10 shrink-0 items-center gap-0.5 px-2 select-none"
      >
        <div className="flex min-w-0 items-center gap-0.5" role="tablist">
          {TABS.map(([id, label, Icon]) => (
            <button
              key={id}
              type="button"
              role="tab"
              aria-selected={tab === id}
              title={label}
              onClick={() => setPrefs({ rightTab: id })}
              className={cn(
                'flex h-7 min-w-0 cursor-pointer items-center gap-1 rounded-md px-1.5 text-base',
                tab === id ? 'bg-card text-foreground' : 'text-quiet hover:bg-hover hover:text-soft'
              )}
            >
              <Icon className="size-3.5 shrink-0" />
              <span className="min-w-0 truncate">{label}</span>
            </button>
          ))}
        </div>
        <div className="flex-1 self-stretch" />
        <RightPanelToggle />
      </div>
      {active ? (
        <PanelBody connectionId={active.connectionId} database={active.database} tab={tab} />
      ) : (
        <Empty icon={Table2}>Select a database.</Empty>
      )}
    </aside>
  )
}

function Empty({ icon: Icon, children }: { icon: typeof Info; children: React.ReactNode }) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-3 px-8 pb-16 text-center text-sm text-quiet">
      <Icon className="size-5 text-dim" />
      <div className="max-w-[260px] leading-relaxed">{children}</div>
    </div>
  )
}

function PanelBody({
  connectionId,
  database,
  tab
}: {
  connectionId: string
  database: string
  tab: RightTab
}) {
  if (tab === 'structure') return <StructureTab connectionId={connectionId} database={database} />
  if (tab === 'history') return <HistoryTab connectionId={connectionId} />
  return <InfoTab connectionId={connectionId} database={database} />
}

function StructureTab({ connectionId, database }: { connectionId: string; database: string }) {
  const key = dbKey(connectionId, database)
  const activeTab = useStore((s) => {
    const ws = s.workspaces[key]
    return ws?.tabs.find((t) => t.id === ws.activeTab)
  })
  // Subscribe to connections so the table re-resolves when they change.
  useStore((s) => s.connections[connectionId])
  if (activeTab?.kind !== 'table') {
    return <Empty icon={Columns3}>Open a table to see its columns.</Empty>
  }
  const table = findTable(connectionId, database, activeTab.schema, activeTab.table)
  if (!table) return <Empty icon={Columns3}>Table not found.</Empty>
  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <div className="flex flex-col gap-2.5 px-3 pt-1.5 pb-3">
        <div className="flex min-w-0 items-center gap-2 text-sm">
          <span
            className={cn(
              'shrink-0 rounded-[5px] px-1.5 py-px text-sm',
              table.kind === 'view' ? 'bg-secondary text-soft' : 'bg-status-run/15 text-link'
            )}
          >
            {table.kind === 'view' ? 'View' : 'Table'}
          </span>
          <span className="min-w-0 truncate font-mono text-xs text-soft">
            {activeTab.schema}.{table.name}
          </span>
        </div>
        <h2 className="min-w-0 text-lg leading-snug font-medium text-foreground">{table.name}</h2>
        <div className="text-sm text-quiet">
          ~{table.rows.toLocaleString()} rows · {bytes(table.sizeBytes)}
        </div>
      </div>
      <Section title="Columns" count={table.columns.length}>
        {table.columns.map((c) => (
          <div
            key={c.name}
            className="flex h-8 items-center gap-2.5 px-3 text-sm"
            title={c.default ? `default ${c.default}` : undefined}
          >
            <span className="grid size-3.5 shrink-0 place-items-center">
              {c.pk && <KeyRound className="size-3.5 text-status-wait" aria-label="primary key" />}
            </span>
            <span className="min-w-0 truncate font-mono text-soft">{c.name}</span>
            <span className="ml-auto shrink-0 font-mono text-xs text-quiet">
              {c.type}
              {c.nullable ? '?' : ''}
            </span>
          </div>
        ))}
      </Section>
    </div>
  )
}

function InfoTab({ connectionId, database }: { connectionId: string; database: string }) {
  const conn = useStore((s) => s.connections[connectionId])
  if (!conn) return null
  const db = conn.databases.find((d) => d.name === database)
  const url = connectionString(conn, database)
  const dot = connDot(conn.state)
  const copy = () => {
    void navigator.clipboard?.writeText(url).catch(() => {})
    pushToast({ kind: 'info', title: `Copied ${url}` }, 3200)
  }
  const tables = db?.schemas.reduce((n, s) => n + s.tables.length, 0) ?? 0
  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <div className="flex flex-col gap-2.5 px-3 pt-1.5 pb-3">
        <div className="flex min-w-0 items-center gap-2 text-sm">
          <span className="shrink-0 rounded-[5px] bg-status-run/15 px-1.5 py-px text-sm text-link">
            {ENGINE_LABEL[conn.engine]}
          </span>
          <span className="min-w-0 truncate font-mono text-xs text-soft">{conn.name}</span>
        </div>
        <div className="flex items-start gap-1.5">
          <h2 className="min-w-0 text-lg leading-snug font-medium text-foreground">{database}</h2>
          <IconButton label="Copy connection URL" side="bottom" className="mt-0.5" onClick={copy}>
            <Copy className="size-3.5" />
          </IconButton>
        </div>
        <div className="flex items-center gap-2 text-sm text-quiet">
          <StatusDot kind={dot} size={6} />
          {DOT_LABEL[dot]}
          {conn.latencyMs != null && conn.engine !== 'sqlite' ? ` · ${conn.latencyMs} ms` : ''}
          {conn.serverVersion ? ` · ${conn.serverVersion}` : ''}
        </div>
      </div>
      <Section title="Schemas" count={db?.schemas.length}>
        {db?.schemas.map((s) => (
          <div key={s.name} className="flex h-8 items-center gap-2.5 px-3 text-sm">
            <span className="min-w-0 truncate font-mono text-soft">{s.name}</span>
            <span className="ml-auto shrink-0 text-xs text-quiet">
              {s.tables.length} table{s.tables.length === 1 ? '' : 's'}
            </span>
          </div>
        ))}
        {tables === 0 && <div className="px-3 py-2 text-sm text-quiet">No tables</div>}
      </Section>
      <Section title="Connection URL">
        <button
          type="button"
          title="Copy URL"
          onClick={copy}
          className="mx-1.5 cursor-copy rounded-md px-1.5 py-1.5 text-left font-mono text-xs break-all text-soft hover:bg-hover"
        >
          {url}
        </button>
      </Section>
    </div>
  )
}

function HistoryTab({ connectionId }: { connectionId: string }) {
  const history = useStore((s) => s.history[connectionId])
  if (!history?.length) {
    return <Empty icon={History}>Queries you run on this connection show up here.</Empty>
  }
  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <Section title="Recent" count={history.length}>
        {history.map((h) => (
          <button
            key={h.id}
            type="button"
            title={`${h.sql}\n\nClick to open in a new query tab`}
            onClick={() => newQueryTab(connectionId, h.database, h.sql)}
            className="flex cursor-pointer flex-col gap-0.5 px-3 py-1.5 text-left hover:bg-hover"
          >
            <span className="line-clamp-2 font-mono text-xs break-all text-soft">{h.sql}</span>
            <span className="text-xs text-dim">
              {h.database} · {ago(h.at)} · {h.ms} ms
              {h.rows != null ? ` · ${h.rows} rows` : ''}
            </span>
          </button>
        ))}
      </Section>
    </div>
  )
}

/** Cursor's "1 File Changed" strip: a hairline-ruled header row above a list. */
function Section({
  title,
  count,
  children
}: {
  title: string
  count?: number
  children: React.ReactNode
}) {
  return (
    <div className="flex flex-col pb-1">
      <div className="flex h-9 items-center gap-1.5 border-y border-border px-3 text-sm text-quiet">
        {title}
        {count != null && count > 0 && <span className="text-dim">{count}</span>}
      </div>
      <div className="flex flex-col py-1">{children}</div>
    </div>
  )
}

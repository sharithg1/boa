// ⌘K palette, also opened from the title bar's database selector and the
// sidebar's Search: recent tables first when the query is empty, fuzzy
// matches on table, schema, database and connection otherwise. ↑↓ / Tab
// move, ⏎ opens (pinned), Esc closes.
import { useEffect, useMemo, useRef, useState } from 'react'
import { Eye, History, Table2 } from 'lucide-react'
import { openTable, tableKey, useStore } from '@/state/store'
import { openHistory } from '@/state/history'
import { usePrefs } from '@/state/prefs'
import { cn } from '@/lib/utils'
import { ago, compactCount, fuzzy, highlightRuns } from '@/lib/format'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { RingGlyph, Tile } from './chrome'

type Item = {
  key: string
  connectionId: string
  connection: string
  database: string
  schema: string
  table: string
  view: boolean
  rows: number
}

type Row = Item & { idxs: number[] }

/** Commands listed after the tables when every typed word starts one of their words. */
type Action = { id: string; label: string; keys: string; words: string[]; run: () => void }
const ACTIONS: Action[] = [
  {
    id: 'history',
    label: 'Query History',
    keys: '⌘⌥E',
    words: ['query', 'history', 'recent', 'statements', 'log', 'sql'],
    run: openHistory
  }
]

function matchActions(q: string): Action[] {
  const terms = q.toLowerCase().split(/\s+/).filter(Boolean)
  if (!terms.length) return []
  return ACTIONS.filter((a) => terms.every((t) => a.words.some((w) => w.startsWith(t))))
}

export function CommandPalette() {
  const open = useStore((s) => s.paletteOpen)
  return (
    <Dialog open={open} onOpenChange={(o) => useStore.setState({ paletteOpen: o })}>
      <DialogContent
        showCloseButton={false}
        overlayClassName="bg-black/50"
        className="top-[13vh] w-[580px] max-w-[calc(100vw-32px)] translate-y-0 gap-0 overflow-hidden rounded-xl p-0 shadow-palette sm:max-w-[580px]"
      >
        <DialogTitle className="sr-only">Open table</DialogTitle>
        <DialogDescription className="sr-only">
          Type to filter by table, schema, database or connection.
        </DialogDescription>
        {open && <PaletteBody />}
      </DialogContent>
    </Dialog>
  )
}

function PaletteBody() {
  const [q, setQ] = useState('')
  const [idx, setIdx] = useState(0)
  const listRef = useRef<HTMLDivElement>(null)
  const connections = useStore((s) => s.connections)
  const order = useStore((s) => s.connectionOrder)
  const mru = useStore((s) => s.mru)
  const seen = useStore((s) => s.seen)
  const recentCount = usePrefs((s) => s.recentCount)

  const items = useMemo(
    () =>
      order.flatMap((id) => {
        const c = connections[id]
        if (c?.state !== 'connected') return []
        return c.databases.flatMap((d) =>
          d.schemas.flatMap((s) =>
            s.tables.map(
              (t): Item => ({
                key: tableKey(id, d.name, s.name, t.name),
                connectionId: id,
                connection: c.name,
                database: d.name,
                schema: s.name,
                table: t.name,
                view: t.kind === 'view',
                rows: t.rows
              })
            )
          )
        )
      }),
    [connections, order]
  )

  const groups = useMemo((): { label: string | null; rows: Row[] }[] => {
    const query = q.trim()
    if (!query) {
      const byKey = new Map(items.map((i) => [i.key, i]))
      const recent = mru
        .map((k) => byKey.get(k))
        .filter((i): i is Item => !!i)
        .slice(0, recentCount)
      const rest = items.filter((i) => !recent.includes(i))
      return [
        ...(recent.length
          ? [{ label: 'Recent', rows: recent.map((r) => ({ ...r, idxs: [] })) }]
          : []),
        { label: recent.length ? 'All tables' : null, rows: rest.map((r) => ({ ...r, idxs: [] })) }
      ].filter((g) => g.rows.length)
    }
    const hits = items
      .map((it) => {
        const name = fuzzy(query, it.table)
        const full = fuzzy(query, `${it.schema}.${it.table} ${it.database} ${it.connection}`)
        const best = name ?? full
        if (!best) return null
        return { ...it, idxs: name?.idxs ?? [], score: (name ? 5 : 0) + best.score }
      })
      .filter((x): x is Row & { score: number } => !!x)
      .sort((a, b) => b.score - a.score)
    return [{ label: null, rows: hits }]
  }, [items, q, mru, recentCount])
  const flat = useMemo(() => groups.flatMap((g) => g.rows), [groups])
  const actions = useMemo(() => matchActions(q), [q])
  const total = flat.length + actions.length

  const sel = Math.min(idx, Math.max(0, total - 1))
  useEffect(() => {
    listRef.current?.querySelector(`[data-pi="${sel}"]`)?.scrollIntoView({ block: 'nearest' })
  }, [sel])

  const choose = (it: Row | undefined) => {
    if (!it) return
    useStore.setState({ paletteOpen: false })
    openTable(it.connectionId, it.database, it.schema, it.table, { pin: true })
  }
  const runAction = (a: Action | undefined) => {
    if (!a) return
    useStore.setState({ paletteOpen: false })
    a.run()
  }
  const move = (d: number) => total && setIdx((sel + d + total) % total)

  let n = 0
  return (
    <>
      <div className="flex h-[46px] items-center gap-2.5 border-b border-border px-3.5">
        <RingGlyph size={11} />
        <input
          autoFocus
          value={q}
          spellCheck={false}
          aria-label="Open table"
          placeholder="Open table…  (table, schema, database or connection)"
          onChange={(e) => {
            setQ(e.target.value)
            setIdx(0)
          }}
          onKeyDown={(e) => {
            if (e.key === 'ArrowDown' || (e.key === 'Tab' && !e.shiftKey)) {
              e.preventDefault()
              move(1)
            } else if (e.key === 'ArrowUp' || (e.key === 'Tab' && e.shiftKey)) {
              e.preventDefault()
              move(-1)
            } else if (e.key === 'Enter') {
              e.preventDefault()
              if (sel < flat.length) choose(flat[sel])
              else runAction(actions[sel - flat.length])
            }
          }}
          className="min-w-0 flex-1 border-none bg-transparent text-base text-foreground outline-none placeholder:text-faint"
        />
        <span className="shrink-0 font-mono text-2xs text-faint">
          {flat.length} table{flat.length === 1 ? '' : 's'}
        </span>
      </div>
      <div ref={listRef} className="max-h-[min(440px,58vh)] overflow-y-auto p-1" role="listbox">
        {groups.map((g, gi) => (
          <div key={`${g.label}:${gi}`} className="pb-1">
            {g.label && (
              <div className="flex items-center gap-[7px] px-2 pt-2 pb-1 text-xs font-medium text-muted-foreground">
                {g.label}
              </div>
            )}
            {g.rows.map((it) => {
              const i = n++
              const Icon = it.view ? Eye : Table2
              return (
                <div
                  key={it.key}
                  data-pi={i}
                  role="option"
                  aria-selected={i === sel}
                  onClick={() => choose(it)}
                  onMouseMove={() => i !== sel && setIdx(i)}
                  className={cn(
                    'flex cursor-default items-center gap-2.5 rounded-md px-2 py-1.5',
                    i === sel && 'bg-accent'
                  )}
                >
                  <Icon className="size-3.5 shrink-0 text-quiet" />
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-[7px]">
                      <span className="min-w-0 truncate font-mono text-sm">
                        <Hl text={it.table} idxs={it.idxs} base="text-body" />
                      </span>
                      {it.view && (
                        <span className="shrink-0 rounded-sm bg-muted px-[5px] text-2xs leading-4 font-medium text-body">
                          view
                        </span>
                      )}
                    </div>
                    <div className="flex min-w-0 items-center gap-1 truncate text-xs leading-4 text-faint">
                      <Tile name={it.connection} size={10} />
                      <span className="shrink-0">{it.connection}</span>
                      <span className="text-dim">·</span>
                      <span className="truncate font-mono">
                        {it.database}/{it.schema}
                      </span>
                    </div>
                  </div>
                  <span className="shrink-0 font-mono text-2xs whitespace-nowrap text-faint">
                    {seen[it.key] ? `${ago(seen[it.key]!)} · ` : ''}
                    {compactCount(it.rows)} rows
                  </span>
                  <span className="w-3.5 shrink-0 text-right font-mono text-2xs text-muted-foreground">
                    {i === sel ? '⏎' : ''}
                  </span>
                </div>
              )
            })}
          </div>
        ))}
        {actions.length > 0 && (
          <div className="pb-1">
            <div className="flex items-center gap-[7px] px-2 pt-2 pb-1 text-xs font-medium text-muted-foreground">
              Actions
            </div>
            {actions.map((a) => {
              const i = n++
              return (
                <div
                  key={a.id}
                  data-pi={i}
                  role="option"
                  aria-selected={i === sel}
                  onClick={() => runAction(a)}
                  onMouseMove={() => i !== sel && setIdx(i)}
                  className={cn(
                    'flex cursor-default items-center gap-2.5 rounded-md px-2 py-1.5',
                    i === sel && 'bg-accent'
                  )}
                >
                  <History className="size-3.5 shrink-0 text-quiet" />
                  <span className="min-w-0 flex-1 truncate text-sm text-body">{a.label}</span>
                  <span className="shrink-0 font-mono text-2xs text-faint">{a.keys}</span>
                  <span className="w-3.5 shrink-0 text-right font-mono text-2xs text-muted-foreground">
                    {i === sel ? '⏎' : ''}
                  </span>
                </div>
              )
            })}
          </div>
        )}
        {total === 0 && (
          <div className="p-[22px] text-center text-sm text-faint">No tables match.</div>
        )}
      </div>
      <div className="flex gap-4 border-t border-border px-3.5 py-2 font-mono text-2xs text-faint">
        <span>↑↓ move</span>
        <span>⏎ open</span>
        <span className="flex-1" />
        <span>esc</span>
      </div>
    </>
  )
}

function Hl({
  text,
  idxs,
  base,
  hit = 'text-foreground font-semibold'
}: {
  text: string
  idxs: number[]
  base: string
  hit?: string
}) {
  if (!idxs.length) return <span className={base}>{text}</span>
  return (
    <>
      {highlightRuns(text, idxs).map((r, i) => (
        <span key={i} className={r.hit ? hit : base}>
          {r.s}
        </span>
      ))}
    </>
  )
}

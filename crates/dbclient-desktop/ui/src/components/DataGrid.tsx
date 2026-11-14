// Result grid, used by table tabs and the Database Output panel (DataGrip's
// data editor): sticky header (name + type, click to sort asc → desc → off,
// drag the right edge to resize), sticky row numbers, `<null>` for nulls,
// a selected cell moved with the arrow keys and copied with ⌘C, and a
// floating row-count pill. Values are coloured by column type (numbers,
// booleans, dates, uuids, JSON token by token); a JSON cell opens in the
// JSON viewer on double-click, Enter or its hover button.
import { useMemo, useRef, useState, type KeyboardEvent, type PointerEvent } from 'react'
import { ArrowDown, ArrowUp, Braces, KeyRound } from 'lucide-react'
import { cn } from '@/lib/utils'
import { pushToast } from '@/state/store'
import type { ResultColumn } from '@/state/types'
import { VALUE_CLASS, valueKind } from '@/lib/values'
import { JsonInline, JsonViewer, type JsonTarget } from './JsonViewer'

type Sort = { col: number; dir: 1 | -1 } | null

function initialWidth(c: ResultColumn, rows: (string | null)[][], i: number): number {
  let len = c.name.length + c.type.length + 4
  for (const r of rows.slice(0, 50)) len = Math.max(len, (r[i] ?? '<null>').length)
  return Math.min(360, Math.max(80, len * 7.8 + 24))
}

export function DataGrid({
  columns,
  rows,
  pk
}: {
  columns: ResultColumn[]
  rows: (string | null)[][]
  /** Primary-key column names, marked with a key in the header. */
  pk?: string[]
}) {
  const [sort, setSort] = useState<Sort>(null)
  const [sel, setSel] = useState<{ r: number; c: number } | null>(null)
  const [widths, setWidths] = useState<number[]>(() =>
    columns.map((c, i) => initialWidth(c, rows, i))
  )
  const [viewing, setViewing] = useState<JsonTarget | null>(null)
  const kinds = useMemo(() => columns.map((c) => valueKind(c.type)), [columns])
  const scroller = useRef<HTMLDivElement>(null)
  const resize = useRef<{ i: number; x: number; w: number } | null>(null)

  const view = useMemo(() => {
    const idx = rows.map((_, i) => i)
    if (!sort) return idx
    const numeric = kinds[sort.col] === 'number'
    return idx.sort((a, b) => {
      const x = rows[a]![sort.col]
      const y = rows[b]![sort.col]
      if (x == null || y == null) return x == null ? (y == null ? 0 : 1) : -1
      const d = numeric ? Number(x) - Number(y) : x.localeCompare(y)
      return d * sort.dir
    })
  }, [rows, kinds, sort])

  /** Opens the JSON viewer on a cell (view coordinates) if it holds JSON. */
  const openJson = (r: number, c: number) => {
    const v = rows[view[r]!]?.[c]
    const col = columns[c]
    if (v == null || !col || kinds[c] !== 'json') return false
    setViewing({ column: col.name, type: col.type, row: r + 1, value: v })
    return true
  }

  const cycleSort = (col: number) =>
    setSort((s) => (s?.col !== col ? { col, dir: 1 } : s.dir === 1 ? { col, dir: -1 } : null))

  const onKeyDown = (e: KeyboardEvent) => {
    if (!sel) return
    if ((e.metaKey || e.ctrlKey) && e.key === 'c') {
      const v = rows[view[sel.r]!]?.[sel.c]
      void navigator.clipboard?.writeText(v ?? '').catch(() => {})
      pushToast({ kind: 'info', title: 'Copied value' }, 1600)
      e.preventDefault()
      return
    }
    if (e.key === 'Enter' && openJson(sel.r, sel.c)) {
      e.preventDefault()
      return
    }
    const move: Record<string, [number, number]> = {
      ArrowUp: [-1, 0],
      ArrowDown: [1, 0],
      ArrowLeft: [0, -1],
      ArrowRight: [0, 1]
    }
    const m = move[e.key]
    if (!m) return
    e.preventDefault()
    const r = Math.max(0, Math.min(view.length - 1, sel.r + m[0]))
    const c = Math.max(0, Math.min(columns.length - 1, sel.c + m[1]))
    setSel({ r, c })
    scroller.current
      ?.querySelector<HTMLElement>(`[data-cell="${r}:${c}"]`)
      ?.scrollIntoView({ block: 'nearest', inline: 'nearest' })
  }

  const onResizeMove = (e: PointerEvent) => {
    const d = resize.current
    if (!d) return
    const w = Math.max(48, d.w + e.clientX - d.x)
    setWidths((ws) => ws.map((x, i) => (i === d.i ? w : x)))
  }

  const total = widths.reduce((a, b) => a + b, 0) + 48
  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
      <div
        ref={scroller}
        role="grid"
        aria-label="Results"
        tabIndex={0}
        onKeyDown={onKeyDown}
        className="min-h-0 flex-1 overflow-auto outline-none"
      >
        <table
          className="border-separate border-spacing-0 font-mono text-sm"
          style={{ tableLayout: 'fixed', width: total }}
        >
          <colgroup>
            <col style={{ width: 48 }} />
            {widths.map((w, i) => (
              <col key={columns[i]?.name ?? i} style={{ width: w }} />
            ))}
          </colgroup>
          <thead>
            <tr>
              <th className="sticky top-0 left-0 z-20 h-8 border-r border-b border-border bg-background" />
              {columns.map((c, i) => (
                <th
                  key={`${c.name}-${i}`}
                  onClick={() => cycleSort(i)}
                  title={`${c.name} ${c.type}\nClick to sort`}
                  className="group sticky top-0 z-10 h-8 cursor-default border-r border-b border-border bg-background px-2.5 text-left font-normal hover:bg-hover"
                >
                  <div className="flex min-w-0 items-center gap-1.5">
                    {pk?.includes(c.name) && (
                      <KeyRound
                        className="size-3 shrink-0 text-status-wait"
                        aria-label="primary key"
                      />
                    )}
                    <span className="min-w-0 truncate text-soft">{c.name}</span>
                    <span className="ml-auto shrink-0 text-2xs text-sql-type">{c.type}</span>
                    {sort?.col === i &&
                      (sort.dir === 1 ? (
                        <ArrowUp className="size-3 shrink-0 text-quiet" />
                      ) : (
                        <ArrowDown className="size-3 shrink-0 text-quiet" />
                      ))}
                  </div>
                  <span
                    aria-hidden
                    onClick={(e) => e.stopPropagation()}
                    onPointerDown={(e) => {
                      e.stopPropagation()
                      e.currentTarget.setPointerCapture(e.pointerId)
                      resize.current = { i, x: e.clientX, w: widths[i]! }
                    }}
                    onPointerMove={onResizeMove}
                    onPointerUp={() => {
                      resize.current = null
                    }}
                    className="absolute inset-y-0 -right-[3px] z-10 w-1.5 cursor-col-resize hover:bg-border-strong"
                  />
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {view.map((ri, r) => {
              const row = rows[ri]!
              const rowSel = sel?.r === r
              return (
                <tr key={ri} className={cn(rowSel ? 'bg-hover' : 'hover:bg-hover')}>
                  <td
                    className={cn(
                      'sticky left-0 h-7 border-r border-b border-border-subtle bg-background px-2 text-right text-2xs',
                      rowSel ? 'text-soft' : 'text-dim'
                    )}
                  >
                    {r + 1}
                  </td>
                  {row.map((v, c) => {
                    const active = rowSel && sel?.c === c
                    const kind = kinds[c]!
                    const json = kind === 'json' && v != null
                    return (
                      <td
                        key={c}
                        data-cell={`${r}:${c}`}
                        onMouseDown={() => setSel({ r, c })}
                        onDoubleClick={() => openJson(r, c)}
                        className={cn(
                          'group/cell h-7 truncate border-r border-b border-border-subtle px-2.5 whitespace-nowrap',
                          kind === 'number' && 'text-right',
                          json && 'relative pr-7',
                          active &&
                            'bg-status-run/25 outline outline-1 -outline-offset-1 outline-status-run',
                          v == null ? 'text-dim italic' : VALUE_CLASS[kind]
                        )}
                      >
                        {v == null ? '<null>' : json ? <JsonInline text={v} /> : v}
                        {json && (
                          <button
                            type="button"
                            aria-label="View JSON"
                            title="View JSON (Enter)"
                            onMouseDown={(e) => e.stopPropagation()}
                            onClick={() => {
                              setSel({ r, c })
                              openJson(r, c)
                            }}
                            className={cn(
                              'absolute top-1/2 right-1 grid size-5 -translate-y-1/2 place-items-center rounded text-quiet hover:bg-secondary hover:text-foreground',
                              active ? 'grid' : 'hidden group-hover/cell:grid'
                            )}
                          >
                            <Braces className="size-3.5" />
                          </button>
                        )}
                      </td>
                    )
                  })}
                </tr>
              )
            })}
          </tbody>
        </table>
        {rows.length === 0 && (
          <div className="px-4 py-6 text-center text-sm text-quiet">No rows</div>
        )}
      </div>
      <div className="pointer-events-none absolute inset-x-0 bottom-3 flex justify-center">
        <span className="pointer-events-auto rounded-md border border-border bg-popover px-2.5 py-0.5 text-xs text-soft shadow-menu">
          {rows.length.toLocaleString()} row{rows.length === 1 ? '' : 's'}
        </span>
      </div>
      <JsonViewer
        target={viewing}
        onClose={() => {
          setViewing(null)
          scroller.current?.focus()
        }}
      />
    </div>
  )
}

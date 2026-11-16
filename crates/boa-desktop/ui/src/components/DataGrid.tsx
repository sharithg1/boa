// Result grid, used by table tabs and the Database Output panel (DataGrip's
// data editor): sticky header (name + type, click to sort asc → desc → off,
// drag the right edge to resize), sticky row numbers, `<null>` for nulls,
// a selected cell moved with the arrow keys and copied with ⌘C, and a
// floating row-count pill. Values are coloured by column type (numbers,
// booleans, dates, uuids, JSON token by token); a JSON cell opens in the
// JSON viewer on double-click, Enter or its hover button.
//
// Selection (DataGrip's): click a cell, drag or ⇧-click / ⇧-arrows for a
// range; click a row number for the row, drag or ⇧-click for a run of rows,
// ⌘-click to add / remove one; ⌘A selects every row, Esc goes back to the
// active cell. ⌘C copies the selection as TSV (one cell: its text); the
// right-click menu's "Copy as" writes it as CSV, Markdown, JSON, SQL, …
// (lib/copyAs.ts).
//
// With `editing` (table tabs of tables with a primary key) it is DataGrip's
// data editor: double-click, Enter or typing edits a cell (⇧↵ opens the
// larger value editor), ⌘⌥N sets NULL, ⌘N adds a row, ⌘⌫ deletes the row,
// ⌘⌥Z reverts, ⌘↵ submits; also from the right-click menu. Changed cells,
// new and deleted rows are tinted; the changes themselves live outside.
import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type PointerEvent } from 'react'
import type { MouseEvent } from 'react'
import { ArrowDown, ArrowUp, Braces, KeyRound } from 'lucide-react'
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger
} from '@/components/ui/context-menu'
import {
  COPY_FORMATS,
  type CopyFormat,
  type CopySelection,
  copyAs,
  copyPlain,
  FORMAT_NAME,
  unavailable
} from '@/lib/copyAs'
import type { RowMark } from '@/lib/gridEdits'
import { cn } from '@/lib/utils'
import { pushToast } from '@/state/store'
import type { ResultColumn } from '@/state/types'
import { VALUE_CLASS, valueKind } from '@/lib/values'
import { JsonInline, JsonViewer, type JsonTarget } from './JsonViewer'
import { ValueEditor, type ValueTarget } from './ValueEditor'

/** Editing hooks of the data editor; `r` is an index into `rows`. */
export type GridEditing = {
  rowMark: (r: number) => RowMark
  cellEdited: (r: number, c: number) => boolean
  /** Shown instead of an untouched new-row value (`<default>`). */
  placeholder: (r: number, c: number) => string | undefined
  set: (r: number, c: number, value: string | null) => void
  revertCell: (r: number, c: number) => void
  revertRow: (r: number) => void
  deleteRow: (r: number) => void
  addRow: () => void
  /** How many new rows there are (a new one gets selected). */
  newRows: number
  /** ⌘↵: preview and submit the changes. */
  submit: () => void
  /** The selected row changed (null: none). */
  onSelectRow?: (r: number | null) => void
}

const mac = () => typeof navigator !== 'undefined' && /mac/i.test(navigator.platform)
const MOD = mac() ? '⌘' : 'Ctrl+'
const ALT = mac() ? '⌥' : 'Alt+'

type Sort = { col: number; dir: 1 | -1 } | null

type Cell = { r: number; c: number }

const span = (a: number, b: number) =>
  Array.from({ length: Math.abs(b - a) + 1 }, (_, i) => Math.min(a, b) + i)

const plural = (n: number, w: string) => `${n.toLocaleString()} ${w}${n === 1 ? '' : 's'}`

/** Sorting done by the query (table tabs' ORDER BY) instead of on the loaded rows. */
export type ServerSort = {
  /** A column's direction and position in the ORDER BY, if it is in it. */
  of: (column: string) => { dir: 'asc' | 'desc'; rank: number } | undefined
  /** Header click; `add` with ⇧ (sort by several columns). */
  onSort: (column: string, add: boolean) => void
}

function initialWidth(c: ResultColumn, rows: (string | null)[][], i: number): number {
  let len = c.name.length + c.type.length + 4
  for (const r of rows.slice(0, 50)) len = Math.max(len, (r[i] ?? '<null>').length)
  return Math.min(360, Math.max(80, len * 7.8 + 24))
}

export function DataGrid({
  columns,
  rows,
  pk,
  firstRow = 1,
  table,
  editing,
  serverSort
}: {
  columns: ResultColumn[]
  rows: (string | null)[][]
  /** Number shown on the first row (page offset + 1). */
  firstRow?: number
  /** Primary-key column names, marked with a key in the header. */
  pk?: string[]
  /** Qualified, quoted table name for "Copy as" SQL (table tabs). */
  table?: string
  /** Makes the grid an editor (table tabs only). */
  editing?: GridEditing
  /** Header clicks sort through the query, not the loaded rows. */
  serverSort?: ServerSort
}) {
  const [sort, setSort] = useState<Sort>(null)
  /** The active cell (view coordinates). */
  const [sel, setSel] = useState<Cell | null>(null)
  /** The other corner of a range (or the first of a run of rows); null: just `sel`. */
  const [anchor, setAnchor] = useState<Cell | null>(null)
  /** Whole rows picked by their numbers (view indices), in place of a range. */
  const [picked, setPicked] = useState<number[] | null>(null)
  const drag = useRef<'cells' | 'rows' | null>(null)
  const [widths, setWidths] = useState<number[]>(() =>
    columns.map((c, i) => initialWidth(c, rows, i))
  )
  const [viewing, setViewing] = useState<JsonTarget | null>(null)
  /** Inline cell editor (view coordinates); `touched` once the text changed. */
  const [editor, setEditor] = useState<{
    r: number
    c: number
    text: string
    touched: boolean
  } | null>(null)
  const [big, setBig] = useState<(ValueTarget & { ri: number; c: number }) | null>(null)
  const kinds = useMemo(() => columns.map((c) => valueKind(c.type)), [columns])
  const scroller = useRef<HTMLDivElement>(null)
  const resize = useRef<{ i: number; x: number; w: number } | null>(null)

  // Rows come sorted by the query when it sorts: shown in their order.
  const localSort = serverSort ? null : sort
  const view = useMemo(() => {
    const idx = rows.map((_, i) => i)
    const sort = localSort
    if (!sort) return idx
    const numeric = kinds[sort.col] === 'number'
    return idx.sort((a, b) => {
      const x = rows[a]![sort.col]
      const y = rows[b]![sort.col]
      if (x == null || y == null) return x == null ? (y == null ? 0 : 1) : -1
      const d = numeric ? Number(x) - Number(y) : x.localeCompare(y)
      return d * sort.dir
    })
  }, [rows, kinds, localSort])

  const selRow = sel ? (view[sel.r] ?? null) : null
  const onSelectRow = editing?.onSelectRow
  useEffect(() => onSelectRow?.(selRow), [onSelectRow, selRow])

  /** Makes a cell the active one; `extend` keeps the anchor (a range to it). */
  const focusCell = (r: number, c: number, extend = false) => {
    setAnchor(extend ? ((picked ? null : anchor) ?? sel ?? { r, c }) : null)
    setPicked(null)
    setSel({ r, c })
  }

  const select = (r: number, c: number, extend = false) => {
    focusCell(r, c, extend)
    scroller.current
      ?.querySelector<HTMLElement>(`[data-cell="${r}:${c}"]`)
      ?.scrollIntoView({ block: 'nearest', inline: 'nearest' })
  }

  // A newly added row (the last of `rows`) gets selected and focused.
  const newRows = editing?.newRows ?? 0
  const prevNewRows = useRef(newRows)
  // biome-ignore lint/correctness/useExhaustiveDependencies: runs when a row is added
  useEffect(() => {
    const grew = newRows > prevNewRows.current
    prevNewRows.current = newRows
    if (!grew) return
    const r = view.indexOf(rows.length - 1)
    if (r < 0) return
    select(r, 0)
    scroller.current?.focus()
  }, [rows])

  const canEdit = (ri: number | undefined) =>
    !!editing && ri != null && editing.rowMark(ri) !== 'deleted'

  /** Opens the larger value editor on a cell (view coordinates). */
  const openBig = (r: number, c: number) => {
    const ri = view[r]
    const col = columns[c]
    if (!canEdit(ri) || !col) return false
    setBig({
      ri: ri!,
      c,
      column: col.name,
      type: col.type,
      row: firstRow + r,
      value: editing!.placeholder(ri!, c) ? null : (rows[ri!]?.[c] ?? null),
      json: kinds[c] === 'json'
    })
    return true
  }

  /** Starts editing a cell in place (`initial`: the key typed to start). */
  const startEdit = (r: number, c: number, initial?: string) => {
    const ri = view[r]
    if (!canEdit(ri)) return false
    const v = editing!.placeholder(ri!, c) ? null : (rows[ri!]?.[c] ?? null)
    if (
      initial === undefined &&
      v != null &&
      (v.includes('\n') || v.length > 200 || kinds[c] === 'json')
    )
      return openBig(r, c)
    focusCell(r, c)
    setEditor({ r, c, text: initial ?? v ?? '', touched: initial !== undefined })
    return true
  }

  const finishEdit = (save: boolean, move?: [number, number]) => {
    const ed = editor
    if (!ed) return
    setEditor(null)
    const ri = view[ed.r]
    if (save && ed.touched && ri != null) editing?.set(ri, ed.c, ed.text)
    if (move)
      select(
        Math.max(0, Math.min(view.length - 1, ed.r + move[0])),
        Math.max(0, Math.min(columns.length - 1, ed.c + move[1]))
      )
    scroller.current?.focus()
  }

  const setNull = (r: number, c: number) => {
    const ri = view[r]
    if (canEdit(ri)) editing!.set(ri!, c, null)
  }

  const revert = (r: number, c: number) => {
    const ri = view[r]
    if (!editing || ri == null) return
    if (editing.cellEdited(ri, c)) editing.revertCell(ri, c)
    else editing.revertRow(ri)
  }

  const addRow = () => editing?.addRow()

  /** Edit shortcuts on the selected cell; true when handled. */
  const editKey = (e: KeyboardEvent, r: number, c: number): boolean => {
    if (!editing) return false
    const mod = e.metaKey || e.ctrlKey
    if (mod && e.altKey && e.code === 'KeyN') setNull(r, c)
    else if (mod && e.altKey && e.code === 'KeyZ') revert(r, c)
    else if ((mod && e.code === 'KeyN') || (e.altKey && e.key === 'Insert')) addRow()
    else if ((mod && e.key === 'Backspace') || e.key === 'Delete') {
      const ri = view[r]
      if (ri != null) editing.deleteRow(ri)
    } else if (mod && e.key === 'Enter') editing.submit()
    else if (e.key === 'Enter' && e.shiftKey) openBig(r, c)
    else if (e.key === 'Enter' || e.key === 'F2') startEdit(r, c)
    else if (e.key.length === 1 && !mod && !e.altKey) startEdit(r, c, e.key)
    else return false
    return true
  }

  /** Opens the JSON viewer on a cell (view coordinates) if it holds JSON. */
  const openJson = (r: number, c: number) => {
    const v = rows[view[r]!]?.[c]
    const col = columns[c]
    if (v == null || !col || kinds[c] !== 'json') return false
    setViewing({ column: col.name, type: col.type, row: firstRow + r, value: v })
    return true
  }

  /** Re-sorting moves rows under a range / picked rows (view indices): drop them. */
  const cycleSort = (col: number) => {
    setAnchor(null)
    setPicked(null)
    setSort((s) => (s?.col !== col ? { col, dir: 1 } : s.dir === 1 ? { col, dir: -1 } : null))
  }
  /** A header's sort arrow (and position, sorting by several columns). */
  const sortOf = (i: number): { dir: 1 | -1; rank: number } | undefined => {
    if (!serverSort) return sort?.col === i ? { dir: sort.dir, rank: 1 } : undefined
    const s = serverSort.of(columns[i]!.name)
    return s && { dir: s.dir === 'asc' ? 1 : -1, rank: s.rank }
  }

  const onKeyDown = (e: KeyboardEvent) => {
    if (editing && !sel && (e.metaKey || e.ctrlKey) && e.code === 'KeyN') {
      e.preventDefault()
      addRow()
      return
    }
    if (!sel || editor) return
    if ((e.metaKey || e.ctrlKey) && e.key === 'c') {
      copy()
      e.preventDefault()
      return
    }
    if ((e.metaKey || e.ctrlKey) && e.key === 'a') {
      e.preventDefault()
      setAnchor(null)
      setPicked(view.map((_, i) => i))
      return
    }
    if (e.key === 'Escape' && (anchor || picked)) {
      e.preventDefault()
      setAnchor(null)
      setPicked(null)
      return
    }
    if (editKey(e, sel.r, sel.c)) {
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
    select(r, c, e.shiftKey)
  }

  // ── Selection and copying ──────────────────────────────────────────────

  /** The selected view rows and columns (a range, or picked rows × every column). */
  const region = useMemo(() => {
    if (!sel) return null
    if (picked) {
      const rs = picked.filter((r) => r < view.length).sort((a, b) => a - b)
      return { rows: rs, cols: columns.map((_, i) => i) }
    }
    const a = anchor ?? sel
    return {
      rows: span(a.r, sel.r).filter((r) => r < view.length),
      cols: span(a.c, sel.c).filter((c) => c < columns.length)
    }
  }, [sel, anchor, picked, view.length, columns])
  const pickedSet = useMemo(() => (picked ? new Set(picked) : null), [picked])
  const multi = !!region && region.rows.length * region.cols.length > 1
  const inRange = (r: number, c: number) => {
    if (!sel || !region) return false
    if (pickedSet) return pickedSet.has(r)
    const a = anchor ?? sel
    return (
      r >= Math.min(a.r, sel.r) &&
      r <= Math.max(a.r, sel.r) &&
      c >= Math.min(a.c, sel.c) &&
      c <= Math.max(a.c, sel.c)
    )
  }

  /** Primary-key column indices, when every key column is in the result. */
  const keyCols = useMemo(() => {
    const idx = (pk ?? []).map((k) => columns.findIndex((c) => c.name === k))
    return idx.length && idx.every((i) => i >= 0) ? idx : null
  }, [pk, columns])

  /** The selection for lib/copyAs: displayed values (edits included). */
  const selection = (withRows = true): CopySelection | null => {
    if (!region?.rows.length || !region.cols.length) return null
    const ris = withRows ? region.rows.map((r) => view[r]!) : []
    return {
      columns: region.cols.map((c) => columns[c]!),
      rows: ris.map((ri) => region.cols.map((c) => rows[ri]?.[c] ?? null)),
      table,
      key: keyCols
        ? {
            columns: keyCols.map((c) => columns[c]!),
            rows: ris.map((ri) => keyCols.map((c) => rows[ri]?.[c] ?? null))
          }
        : undefined,
      isDefault: editing ? (i, j) => !!editing.placeholder(ris[i]!, region.cols[j]!) : undefined
    }
  }

  /** Copies the selection: TSV (⌘C), or `format` from "Copy as". */
  const copy = (format?: CopyFormat) => {
    const s = selection()
    if (!s) return
    let text: string
    try {
      text = format ? copyAs(format, s) : copyPlain(s)
    } catch (err) {
      pushToast({ kind: 'error', title: 'Copy failed', body: String(err) })
      return
    }
    void navigator.clipboard?.writeText(text).catch(() => {})
    const n = s.rows.length
    const title =
      !format && n === 1 && s.columns.length === 1
        ? 'Copied value'
        : format === 'sql-in'
          ? `Copied ${plural(n, 'value')} as an SQL IN list`
          : `Copied ${plural(n, 'row')}${format ? ` as ${FORMAT_NAME[format]}` : ''}`
    pushToast({ kind: 'info', title }, 1600)
  }

  /** Mouse down on a cell: select it, extend to it (⇧) or start a drag. */
  const cellDown = (e: MouseEvent, r: number, c: number) => {
    // Right-click inside the selection keeps it for the context menu.
    if (e.button === 2 && inRange(r, c)) return
    if (e.button === 0 && e.shiftKey && sel) {
      e.preventDefault()
      scroller.current?.focus()
      focusCell(r, c, true)
      return
    }
    focusCell(r, c)
    setAnchor({ r, c })
    drag.current = e.button === 0 ? 'cells' : null
  }

  /** Mouse down on a row number: pick the row, a run to it (⇧), or toggle it (⌘). */
  const rowDown = (e: MouseEvent, r: number) => {
    const c = sel?.c ?? 0
    if (e.button === 2 && pickedSet?.has(r)) return
    e.preventDefault()
    scroller.current?.focus()
    drag.current = null
    if (e.button === 0 && e.shiftKey && sel) {
      setPicked(span(anchor?.r ?? sel.r, r))
      setAnchor((a) => a ?? sel)
    } else if (e.button === 0 && (e.metaKey || e.ctrlKey)) {
      const base = picked ?? region?.rows ?? []
      setPicked(base.includes(r) ? base.filter((x) => x !== r) : [...base, r])
      setAnchor({ r, c })
    } else {
      setPicked([r])
      setAnchor({ r, c })
      if (e.button === 0) drag.current = 'rows'
    }
    setSel({ r, c })
  }

  /** Dragging over a cell (or row number: `c` null) extends the selection. */
  const dragOver = (e: MouseEvent, r: number, c: number | null) => {
    if (!(e.buttons & 1)) {
      drag.current = null
      return
    }
    if (drag.current === 'cells' && c != null) setSel({ r, c })
    else if (drag.current === 'rows' && anchor) {
      setPicked(span(anchor.r, r))
      setSel({ r, c: sel?.c ?? 0 })
    }
  }

  const onResizeMove = (e: PointerEvent) => {
    const d = resize.current
    if (!d) return
    const w = Math.max(48, d.w + e.clientX - d.x)
    setWidths((ws) => ws.map((x, i) => (i === d.i ? w : x)))
  }

  const total = widths.reduce((a, b) => a + b, 0) + 48
  const selMark = editing && selRow != null ? editing.rowMark(selRow) : undefined
  const body = (
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
                  onClick={(e) =>
                    serverSort ? serverSort.onSort(c.name, e.shiftKey) : cycleSort(i)
                  }
                  title={`${c.name} ${c.type}\nClick to sort${serverSort ? ' (⇧: by several columns)' : ''}`}
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
                    {sortOf(i) &&
                      (sortOf(i)!.dir === 1 ? (
                        <ArrowUp className="size-3 shrink-0 text-quiet" />
                      ) : (
                        <ArrowDown className="size-3 shrink-0 text-quiet" />
                      ))}
                    {(sortOf(i)?.rank ?? 0) > 1 && (
                      <span className="-ml-1 shrink-0 text-2xs text-dim">{sortOf(i)!.rank}</span>
                    )}
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
              const rowPicked = multi && (pickedSet ? pickedSet.has(r) : inRange(r, sel!.c))
              const mark = editing?.rowMark(ri)
              return (
                <tr
                  key={ri}
                  className={cn(
                    rowSel ? 'bg-hover' : 'hover:bg-hover',
                    mark === 'inserted' && 'bg-status-ok/10',
                    mark === 'deleted' && 'bg-status-err/10 line-through decoration-status-err/70'
                  )}
                >
                  <td
                    onMouseDown={(e) => rowDown(e, r)}
                    onMouseEnter={(e) => dragOver(e, r, null)}
                    title={
                      mark === 'inserted'
                        ? 'New row'
                        : mark === 'deleted'
                          ? 'Deleted row'
                          : mark === 'updated'
                            ? 'Changed row'
                            : undefined
                    }
                    className={cn(
                      'sticky left-0 h-7 border-r border-b border-border-subtle bg-background px-2 text-right text-2xs',
                      rowSel ? 'text-soft' : 'text-dim',
                      rowPicked && 'bg-status-run/15 text-soft',
                      mark === 'inserted' && 'text-status-ok',
                      mark === 'deleted' && 'text-status-err',
                      mark === 'updated' && 'text-status-run'
                    )}
                  >
                    {mark === 'inserted' ? '+' : firstRow + r}
                  </td>
                  {row.map((v, c) => {
                    const active = rowSel && sel?.c === c
                    const kind = kinds[c]!
                    const placeholder = editing?.placeholder(ri, c)
                    const json = kind === 'json' && v != null && !placeholder
                    const edited = editing?.cellEdited(ri, c)
                    const inline = editor && editor.r === r && editor.c === c
                    return (
                      <td
                        key={c}
                        data-cell={`${r}:${c}`}
                        onMouseDown={(e) => cellDown(e, r, c)}
                        onMouseEnter={(e) => dragOver(e, r, c)}
                        onDoubleClick={() => (editing ? startEdit(r, c) : openJson(r, c))}
                        className={cn(
                          'group/cell h-7 truncate border-r border-b border-border-subtle px-2.5 whitespace-nowrap',
                          kind === 'number' && 'text-right',
                          (json || inline) && 'relative pr-7',
                          edited && mark !== 'inserted' && 'bg-status-run/20',
                          multi && !active && inRange(r, c) && 'bg-status-run/15',
                          active &&
                            'bg-status-run/25 outline outline-1 -outline-offset-1 outline-status-run',
                          v == null || placeholder ? 'text-dim italic' : VALUE_CLASS[kind]
                        )}
                      >
                        {inline && (
                          <input
                            autoFocus
                            spellCheck={false}
                            aria-label={`Edit ${columns[c]?.name}`}
                            value={editor.text}
                            placeholder={v == null ? '<null>' : ''}
                            onFocus={(e) => {
                              if (!editor.touched) e.currentTarget.select()
                            }}
                            onChange={(e) =>
                              setEditor({ ...editor, text: e.target.value, touched: true })
                            }
                            onBlur={() => finishEdit(true)}
                            onKeyDown={(e) => {
                              e.stopPropagation()
                              const mod = e.metaKey || e.ctrlKey
                              if (mod && e.altKey && e.code === 'KeyN') {
                                e.preventDefault()
                                setEditor(null)
                                setNull(r, c)
                                scroller.current?.focus()
                              } else if (e.key === 'Enter' && e.shiftKey) {
                                e.preventDefault()
                                setEditor(null)
                                openBig(r, c)
                              } else if (e.key === 'Enter') {
                                e.preventDefault()
                                finishEdit(true)
                              } else if (e.key === 'Escape') {
                                e.preventDefault()
                                finishEdit(false)
                              } else if (e.key === 'Tab') {
                                e.preventDefault()
                                finishEdit(true, [0, e.shiftKey ? -1 : 1])
                              }
                            }}
                            className="absolute inset-0 z-10 bg-field px-2.5 font-mono text-sm text-body no-underline outline outline-2 -outline-offset-2 outline-status-run placeholder:text-dim placeholder:italic"
                          />
                        )}
                        {placeholder ?? (v == null ? '<null>' : json ? <JsonInline text={v} /> : v)}
                        {json && (
                          <button
                            type="button"
                            aria-label="View JSON"
                            title="View JSON (Enter)"
                            onMouseDown={(e) => e.stopPropagation()}
                            onClick={() => {
                              focusCell(r, c)
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
    </div>
  )
  // Outside the menu trigger: right-clicks in these dialogs aren't the grid's.
  const dialogs = (
    <>
      <ValueEditor
        target={big}
        onApply={(v) => {
          if (big && v !== (editing?.placeholder(big.ri, big.c) ? null : rows[big.ri]?.[big.c]))
            editing?.set(big.ri, big.c, v)
          setBig(null)
          scroller.current?.focus()
        }}
        onClose={() => {
          setBig(null)
          scroller.current?.focus()
        }}
      />
      <JsonViewer
        target={viewing}
        onClose={() => {
          setViewing(null)
          scroller.current?.focus()
        }}
      />
    </>
  )
  const shape = selection(false)
  const copyMenu = (
    <>
      <ContextMenuItem disabled={!shape} onSelect={() => copy()}>
        Copy<ContextMenuShortcut>{`${MOD}C`}</ContextMenuShortcut>
      </ContextMenuItem>
      <ContextMenuSub>
        <ContextMenuSubTrigger disabled={!shape}>Copy as</ContextMenuSubTrigger>
        <ContextMenuSubContent className="w-[190px]">
          {COPY_FORMATS.map(({ format, label }) => {
            const why = shape && unavailable(format, shape)
            return (
              <ContextMenuItem
                key={format}
                disabled={!shape || !!why}
                title={why || undefined}
                onSelect={() => copy(format)}
              >
                {label}
              </ContextMenuItem>
            )
          })}
        </ContextMenuSubContent>
      </ContextMenuSub>
    </>
  )
  return (
    <>
      <ContextMenu>
        <ContextMenuTrigger asChild>{body}</ContextMenuTrigger>
        <ContextMenuContent className="w-[220px]">
          {copyMenu}
          {editing && <ContextMenuSeparator />}
          {editing && editMenu(editing)}
        </ContextMenuContent>
      </ContextMenu>
      {dialogs}
    </>
  )

  function editMenu(editing: GridEditing) {
    return (
      <>
        <ContextMenuItem
          disabled={!sel || selMark === 'deleted'}
          onSelect={() => sel && startEdit(sel.r, sel.c)}
        >
          Edit<ContextMenuShortcut>↵</ContextMenuShortcut>
        </ContextMenuItem>
        <ContextMenuItem
          disabled={!sel || selMark === 'deleted'}
          onSelect={() => sel && openBig(sel.r, sel.c)}
        >
          Open in editor<ContextMenuShortcut>⇧↵</ContextMenuShortcut>
        </ContextMenuItem>
        <ContextMenuItem
          disabled={!sel || selMark === 'deleted'}
          onSelect={() => sel && setNull(sel.r, sel.c)}
        >
          Set NULL<ContextMenuShortcut>{`${MOD}${ALT}N`}</ContextMenuShortcut>
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem onSelect={addRow}>
          Add row<ContextMenuShortcut>{`${MOD}N`}</ContextMenuShortcut>
        </ContextMenuItem>
        <ContextMenuItem
          disabled={selRow == null || selMark === 'deleted'}
          onSelect={() => selRow != null && editing.deleteRow(selRow)}
        >
          Delete row<ContextMenuShortcut>{`${MOD}⌫`}</ContextMenuShortcut>
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem
          disabled={!sel || selRow == null || !editing.cellEdited(selRow, sel.c)}
          onSelect={() => sel && selRow != null && editing.revertCell(selRow, sel.c)}
        >
          Revert cell
        </ContextMenuItem>
        <ContextMenuItem
          disabled={selRow == null || !selMark}
          onSelect={() => selRow != null && editing.revertRow(selRow)}
        >
          Revert row<ContextMenuShortcut>{`${MOD}${ALT}Z`}</ContextMenuShortcut>
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem onSelect={editing.submit}>
          Preview and submit…<ContextMenuShortcut>{`${MOD}↵`}</ContextMenuShortcut>
        </ContextMenuItem>
      </>
    )
  }
}

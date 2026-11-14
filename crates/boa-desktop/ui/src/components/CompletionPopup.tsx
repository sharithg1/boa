// Completion list for the SQL console, positioned under the word being
// completed. The textarea keeps focus throughout: the editor routes arrow /
// Enter / Tab / Escape here while the list is open, and a click picks an item
// without blurring the textarea (mousedown is cancelled).
import { useEffect, useRef } from 'react'
import {
  Braces,
  Columns3,
  Database,
  Eye,
  KeyRound,
  LetterText,
  SquareFunction,
  Table2,
  Type,
  type LucideIcon
} from 'lucide-react'
import type { Completion, CompletionKind } from '@/lib/complete'
import { cn } from '@/lib/utils'

const ICON: Record<CompletionKind, [LucideIcon, string]> = {
  keyword: [LetterText, 'text-sql-keyword'],
  schema: [Database, 'text-sql-table'],
  table: [Table2, 'text-sql-table'],
  view: [Eye, 'text-sql-table'],
  cte: [Braces, 'text-sql-table'],
  column: [Columns3, 'text-sql-column'],
  alias: [Table2, 'text-quiet'],
  function: [SquareFunction, 'text-sql-function'],
  type: [Type, 'text-sql-type']
}

/** Row height; the list shows at most VISIBLE rows before scrolling. */
const ROW = 22
const VISIBLE = 10
/** Icon column, so labels line up with the word being completed. */
const GUTTER = 26

export type CaretBox = { x: number; y: number; lineHeight: number }

/** Where `pos` sits inside a no-wrap monospace textarea, relative to its box. */
export function caretBox(el: HTMLTextAreaElement, pos: number): CaretBox {
  const cs = getComputedStyle(el)
  const before = el.value.slice(0, pos)
  const lineStart = before.lastIndexOf('\n') + 1
  const line = before.split('\n').length - 1
  const tab = Number.parseInt(cs.tabSize, 10) || 8
  let col = 0
  for (const ch of before.slice(lineStart))
    col = ch === '\t' ? (Math.floor(col / tab) + 1) * tab : col + 1
  const lineHeight = Number.parseFloat(cs.lineHeight) || 20
  return {
    x: Number.parseFloat(cs.paddingLeft) + col * charWidth(cs) - el.scrollLeft,
    y: Number.parseFloat(cs.paddingTop) + line * lineHeight - el.scrollTop,
    lineHeight
  }
}

let measure: CanvasRenderingContext2D | null = null
const widths = new Map<string, number>()
export function charWidth(cs: CSSStyleDeclaration): number {
  const font = `${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}`
  let w = widths.get(font)
  if (w === undefined) {
    measure ??= document.createElement('canvas').getContext('2d')
    if (!measure) return 7.8
    measure.font = font
    w = measure.measureText('x'.repeat(100)).width / 100
    // Before the web font loads the fallback's width is wrong; don't cache it.
    if (document.fonts.status === 'loaded') widths.set(font, w)
  }
  return w
}

export function CompletionPopup({
  items,
  selected,
  caret,
  bounds,
  onPick,
  onHover
}: {
  items: Completion[]
  selected: number
  caret: CaretBox
  /** Size of the editor area the list must stay inside. */
  bounds: { width: number; height: number }
  onPick: (i: number) => void
  onHover: (i: number) => void
}) {
  const list = useRef<HTMLDivElement>(null)

  useEffect(() => {
    list.current?.querySelector(`[data-ci="${selected}"]`)?.scrollIntoView({ block: 'nearest' })
  }, [selected])

  const height = Math.min(items.length, VISIBLE) * ROW + 8
  const below = caret.y + caret.lineHeight
  // Flip above the line when there is no room below.
  const top = below + height > bounds.height && caret.y - height > 0 ? caret.y - height : below
  const width = 380
  const left = Math.max(4, Math.min(caret.x - GUTTER, bounds.width - width - 4))

  return (
    <div
      ref={list}
      role="listbox"
      aria-label="Completions"
      onMouseDown={(e) => e.preventDefault()}
      style={{ top, left, width, maxHeight: height }}
      className="absolute z-20 overflow-y-auto rounded-md border border-border bg-popover p-1 text-popover-foreground shadow-palette"
    >
      {items.map((it, i) => {
        const [Icon, tint] = it.pk ? [KeyRound, 'text-sql-number'] : ICON[it.kind]
        return (
          <div
            key={`${it.kind}:${it.label}:${it.detail ?? ''}`}
            data-ci={i}
            role="option"
            aria-selected={i === selected}
            onMouseMove={() => i !== selected && onHover(i)}
            onClick={() => onPick(i)}
            style={{ height: ROW }}
            className={cn(
              'flex cursor-default items-center gap-2 rounded-sm px-1.5 font-mono text-sm',
              i === selected && 'bg-accent'
            )}
          >
            <Icon className={cn('size-3.5 shrink-0', tint)} />
            <span className="min-w-0 truncate text-foreground">{it.label}</span>
            <span className="flex-1" />
            {it.detail && (
              <span className="min-w-0 shrink truncate text-xs text-faint">{it.detail}</span>
            )}
          </div>
        )
      })}
    </div>
  )
}

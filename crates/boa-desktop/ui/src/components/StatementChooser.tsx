// ⌘↵ chooser for the SQL console (DataGrip's "Statements" popup): when the
// caret sits in a subquery, pick the whole statement or the inner query.
// Like the completion list, the textarea keeps focus: the editor routes the
// arrows, Enter and Escape here, and the highlighted entry is filled in the
// editor so you can see exactly what will run.
import type { Range } from '@/lib/statements'
import { preview } from '@/lib/statements'
import { cn } from '@/lib/utils'
import type { CaretBox } from './CompletionPopup'

export function StatementChooser({
  sql,
  items,
  selected,
  caret,
  bounds,
  onPick,
  onHover
}: {
  sql: string
  items: Range[]
  selected: number
  caret: CaretBox
  bounds: { width: number; height: number }
  onPick: (i: number) => void
  onHover: (i: number) => void
}) {
  const width = 400
  const height = items.length * 26 + 40
  const below = caret.y + caret.lineHeight + 2
  const top = below + height > bounds.height && caret.y - height > 0 ? caret.y - height - 2 : below
  const left = Math.max(4, Math.min(caret.x - 12, bounds.width - width - 4))
  return (
    <div
      role="listbox"
      aria-label="Statements"
      onMouseDown={(e) => e.preventDefault()}
      style={{ top, left, width }}
      className="absolute z-20 rounded-md border border-border bg-popover p-1 text-popover-foreground shadow-palette"
    >
      <div className="px-2 pt-1 pb-1.5 text-center text-xs font-medium text-soft">Statements</div>
      {items.map((r, i) => (
        <div
          key={`${r.from}-${r.to}`}
          role="option"
          aria-selected={i === selected}
          onMouseMove={() => i !== selected && onHover(i)}
          onClick={() => onPick(i)}
          className={cn(
            'flex h-[26px] cursor-default items-center rounded-sm px-2 font-mono text-sm',
            i === selected ? 'bg-status-run/30 text-foreground' : 'text-soft'
          )}
        >
          <span className="min-w-0 truncate">{preview(sql, r)}</span>
        </div>
      ))}
    </div>
  )
}

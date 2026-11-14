// DataGrip's "N of M" badge on a connection (databases) or a database
// (schemas): click for a checklist of what the explorer shows and
// introspects. A filter box, an "All" row (also covers ones created later),
// the default item tagged; Enter or a click outside applies, Esc cancels.
import { useMemo, useRef, useState } from 'react'
import { Search } from 'lucide-react'
import { Checkbox } from '@/components/ui/checkbox'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { cn } from '@/lib/utils'
import type { Scope } from '@/state/store'

export function ScopePicker({
  kind,
  items,
  selected,
  isAll = false,
  defaultItem,
  onApply
}: {
  kind: 'databases' | 'schemas'
  /** Every name, in display order. */
  items: string[]
  /** Currently shown names (resolved, so the default counts). */
  selected: string[]
  /** The saved scope is '*'. */
  isAll?: boolean
  defaultItem?: string
  onApply: (scope: Scope) => void
}) {
  const [open, setOpen] = useState(false)
  const [all, setAll] = useState(false)
  const [draft, setDraft] = useState<Set<string>>(new Set())
  const [filter, setFilter] = useState('')
  const cancelled = useRef(false)
  const savedAll = useRef(false)

  const shown = useMemo(() => {
    const q = filter.trim().toLowerCase()
    return q ? items.filter((i) => i.toLowerCase().includes(q)) : items
  }, [items, filter])

  const isOn = (name: string) => all || draft.has(name)
  const count = all ? items.length : items.filter((i) => draft.has(i)).length

  const toggle = (name: string) => {
    // Unticking one while "All" is on keeps the rest ticked.
    const base = all ? new Set(items) : new Set(draft)
    if (base.has(name)) base.delete(name)
    else base.add(name)
    setAll(false)
    setDraft(base)
  }

  const openChange = (next: boolean) => {
    if (next) {
      savedAll.current = isAll
      setAll(isAll)
      setDraft(new Set(selected))
      setFilter('')
      cancelled.current = false
      setOpen(true)
      return
    }
    setOpen(false)
    if (cancelled.current) return
    const changed =
      all !== savedAll.current ||
      (!all && (draft.size !== selected.length || selected.some((s) => !draft.has(s))))
    if (changed) onApply(all ? '*' : items.filter((i) => draft.has(i)))
  }

  return (
    <Popover open={open} onOpenChange={openChange}>
      <PopoverTrigger
        onClick={(e) => e.stopPropagation()}
        onDoubleClick={(e) => e.stopPropagation()}
        title={`Choose ${kind} to show`}
        className="shrink-0 cursor-pointer rounded-[4px] border border-border px-1 text-xs leading-4 text-dim outline-none hover:border-border-strong hover:text-soft data-[state=open]:border-border-strong data-[state=open]:text-soft"
      >
        {selected.length} of {items.length}
      </PopoverTrigger>
      <PopoverContent
        align="start"
        sideOffset={4}
        onClick={(e) => e.stopPropagation()}
        onDoubleClick={(e) => e.stopPropagation()}
        onEscapeKeyDown={() => {
          cancelled.current = true
        }}
        // Enter applies from anywhere in the list (filter box or a checkbox).
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault()
            openChange(false)
          }
        }}
        className="flex w-[340px] flex-col gap-0 p-0"
      >
        <label className="flex h-9 shrink-0 items-center gap-2 border-b border-border px-3">
          <Search className="size-3.5 shrink-0 text-dim" />
          <input
            autoFocus
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder={`Filter ${kind}`}
            className="min-w-0 flex-1 bg-transparent text-sm text-body outline-none placeholder:text-dim"
          />
          <span className="shrink-0 text-xs text-dim">
            {count} of {items.length}
          </span>
        </label>
        <div className="max-h-[360px] overflow-y-auto py-1">
          {!filter && (
            <Row
              checked={all ? true : count > 0 ? 'indeterminate' : false}
              onToggle={() => {
                if (all) {
                  setAll(false)
                  setDraft(new Set())
                } else setAll(true)
              }}
              className="border-b border-border-subtle"
            >
              All {kind}
              <span className="text-xs text-dim">includes new ones</span>
            </Row>
          )}
          {shown.map((name) => (
            <Row key={name} checked={isOn(name)} onToggle={() => toggle(name)}>
              <span className="truncate font-mono">{name}</span>
              {name === defaultItem && <span className="shrink-0 text-xs text-dim">(default)</span>}
            </Row>
          ))}
          {shown.length === 0 && <div className="px-3 py-3 text-sm text-dim">No {kind} match</div>}
        </div>
        <div className="flex h-8 shrink-0 items-center border-t border-border px-3 text-xs text-dim">
          Enter or click outside to apply · Esc cancels
        </div>
      </PopoverContent>
    </Popover>
  )
}

function Row({
  checked,
  onToggle,
  className,
  children
}: {
  checked: boolean | 'indeterminate'
  onToggle: () => void
  className?: string
  children: React.ReactNode
}) {
  return (
    <div
      onClick={onToggle}
      className={cn(
        'flex h-7 cursor-default items-center gap-2 px-3 text-sm text-soft hover:bg-hover',
        className
      )}
    >
      <Checkbox
        checked={checked}
        onClick={(e) => e.stopPropagation()}
        onCheckedChange={onToggle}
        className="shrink-0"
      />
      {children}
    </div>
  )
}

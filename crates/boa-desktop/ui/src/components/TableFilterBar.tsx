// DataGrip's filter bar above a table tab's data: a free-form WHERE
// condition and ORDER BY, applied server-side (lib/tableQuery.ts builds the
// SELECT). Enter applies both fields (an emptied one is removed), Escape
// puts back the applied text. Header clicks rewrite the ORDER BY, which
// then shows here.
import { useEffect, useState } from 'react'
import { X } from 'lucide-react'
import type { TableQuery } from '@/lib/tableQuery'
import { cn } from '@/lib/utils'

export function TableFilterBar({
  where,
  orderBy,
  disabled,
  onApply
}: {
  /** The applied clauses. */
  where: string
  orderBy: string
  disabled?: boolean
  onApply: (q: TableQuery) => void
}) {
  const [draft, setDraft] = useState({ where, orderBy })
  // The applied clauses changed (a header click, a restore): show them.
  useEffect(() => setDraft({ where, orderBy }), [where, orderBy])

  const field = (name: keyof typeof draft, label: string, placeholder: string) => (
    <label className="flex h-6 min-w-0 flex-1 items-center gap-2 rounded-md border border-input bg-field px-2">
      <span className="shrink-0 font-mono text-2xs font-medium tracking-wide text-dim">
        {label}
      </span>
      <input
        value={draft[name]}
        disabled={disabled}
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        aria-label={label}
        placeholder={placeholder}
        onChange={(e) => setDraft({ ...draft, [name]: e.target.value })}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault()
            onApply(draft)
          } else if (e.key === 'Escape') {
            e.preventDefault()
            setDraft({ ...draft, [name]: name === 'where' ? where : orderBy })
          }
        }}
        className={cn(
          'min-w-0 flex-1 bg-transparent font-mono text-xs text-body outline-none placeholder:text-dim',
          // Typed but not applied yet.
          draft[name] !== (name === 'where' ? where : orderBy) && 'text-status-run'
        )}
      />
      {draft[name] && !disabled && (
        <button
          type="button"
          title={`Clear ${label}`}
          className="shrink-0 cursor-pointer rounded-sm text-dim hover:text-foreground"
          onClick={() => onApply({ ...draft, [name]: '' })}
        >
          <X className="size-3" />
        </button>
      )}
    </label>
  )

  return (
    <div className="flex h-9 shrink-0 items-center gap-2 border-b border-border px-2">
      {field('where', 'WHERE', "e.g. status = 'active' and id > 10")}
      {field('orderBy', 'ORDER BY', 'e.g. created_at desc, id')}
    </div>
  )
}

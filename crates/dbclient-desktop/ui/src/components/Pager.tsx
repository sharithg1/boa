// DataGrip-style result paging: first / previous / next page around the
// shown row range ("501–1,000"). Pages are stateless LIMIT/OFFSET re-runs,
// so there is no total and no "last page".
import { ChevronLeft, ChevronRight, ChevronsLeft } from 'lucide-react'
import { IconButton } from './IconButton'

export function Pager({
  offset,
  count,
  pageSize,
  more,
  busy,
  onPage
}: {
  /** Rows skipped before this page. */
  offset: number
  /** Rows on this page. */
  count: number
  pageSize: number
  /** More rows exist after this page. */
  more: boolean
  busy?: boolean
  onPage: (offset: number) => void
}) {
  if (offset === 0 && !more) return null
  const range = count
    ? `${(offset + 1).toLocaleString()}–${(offset + count).toLocaleString()}`
    : `${offset.toLocaleString()}+`
  return (
    <div className="flex shrink-0 items-center gap-0.5">
      <IconButton
        label="First page"
        side="top"
        disabled={busy || offset === 0}
        onClick={() => onPage(0)}
      >
        <ChevronsLeft className="size-3.5" />
      </IconButton>
      <IconButton
        label="Previous page"
        side="top"
        disabled={busy || offset === 0}
        onClick={() => onPage(Math.max(0, offset - pageSize))}
      >
        <ChevronLeft className="size-3.5" />
      </IconButton>
      <span className="min-w-16 px-1 text-center font-mono text-xs text-soft tabular-nums">
        {range}
        {more ? ' …' : ''}
      </span>
      <IconButton
        label="Next page"
        side="top"
        disabled={busy || !more}
        onClick={() => onPage(offset + pageSize)}
      >
        <ChevronRight className="size-3.5" />
      </IconButton>
    </div>
  )
}

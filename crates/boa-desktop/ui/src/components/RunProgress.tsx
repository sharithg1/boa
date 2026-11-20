// Feedback while a statement is in flight: a thin indeterminate bar that
// sweeps along the Database Output header (where the result lands), and a
// spinner with the elapsed time for the console toolbar.
// Both fade in after a beat (main.css), so statements that return at once
// don't flash them.
import { useEffect, useState } from 'react'
import { elapsed } from '@/lib/format'
import { cn } from '@/lib/utils'
import { Spinner } from '@/components/ui/spinner'

/** Sweeping bar on the bottom edge of its (relative) parent. */
export function RunBar({ className }: { className?: string }) {
  return <div aria-hidden className={cn('run-bar -bottom-px', className)} />
}

/** "Executing… 1.2 s", counting from `since` (ms epoch). */
export function RunStatus({ since, className }: { since: number; className?: string }) {
  const [now, setNow] = useState(Date.now)
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 100)
    return () => clearInterval(t)
  }, [])
  return (
    <span
      role="status"
      className={cn(
        'run-fade-in flex shrink-0 items-center gap-1.5 text-xs whitespace-nowrap text-dim',
        className
      )}
    >
      <Spinner aria-hidden className="size-3 text-status-run" />
      Executing…
      <span className="tabular-nums">{elapsed(now - since)}</span>
    </span>
  )
}

// Feedback while a statement is in flight: a thin indeterminate bar that
// sweeps along the Database Output header (where the result lands), and a
// spinner with the elapsed time for the console toolbar; and, when it fails,
// the error in a strip under the editor.
// Both fade in after a beat (main.css), so statements that return at once
// don't flash them.
import { useEffect, useState } from 'react'
import { CircleAlert, X } from 'lucide-react'
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

/**
 * The tab's last statement failed: its error (and the server's detail / hint)
 * in a strip under the editor, right above the Database Output panel.
 */
export function RunFailed({
  error,
  detail,
  onDismiss
}: {
  error: string
  detail?: string
  onDismiss: () => void
}) {
  return (
    <div
      role="alert"
      className="flex max-h-32 shrink-0 items-start gap-2 overflow-y-auto border-t border-status-err/40 bg-status-err/10 py-1.5 pr-1.5 pl-3 font-mono text-sm leading-5 select-text"
    >
      <CircleAlert className="mt-0.5 size-4 shrink-0 text-status-err" />
      <div className="min-w-0 flex-1 break-words whitespace-pre-wrap">
        <div className="text-status-err-text">{error}</div>
        {detail && <div className="text-status-err-text/80">{detail}</div>}
      </div>
      <button
        type="button"
        aria-label="Dismiss error"
        onClick={onDismiss}
        className="grid size-5 shrink-0 cursor-pointer place-items-center rounded-[4px] text-status-err-text/70 hover:bg-status-err/20 hover:text-status-err-text"
      >
        <X className="size-3.5" />
      </button>
    </div>
  )
}

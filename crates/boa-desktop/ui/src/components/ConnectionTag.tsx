// A connection's colour, environment and read-only flag where statements
// can run: the explorer row, the editor (stripe), the console toolbar and
// the status bar. Colours come from lib/connTag.ts.
import { Lock } from 'lucide-react'
import { ENV_BADGE, connectionColor } from '@/lib/connTag'
import { cn } from '@/lib/utils'
import { useStore } from '@/state/store'
import type { Connection } from '@/state/types'

/** Environment badge (`PROD`) in the connection colour, plus a lock when read-only. */
export function ConnectionTag({ conn, className }: { conn: Connection; className?: string }) {
  const color = connectionColor(conn)
  if (!conn.environment && !conn.readOnly && !color) return null
  return (
    <span className={cn('flex shrink-0 items-center gap-1', className)}>
      {conn.environment ? (
        <span
          className="rounded-[3px] px-1 py-px font-mono text-[9.5px] leading-none font-semibold tracking-wide text-white"
          style={{ background: color ?? undefined }}
          title={`${conn.environment} environment`}
        >
          {ENV_BADGE[conn.environment]}
        </span>
      ) : color ? (
        <span
          aria-hidden
          className="inline-block size-2 shrink-0 rounded-full"
          style={{ background: color }}
        />
      ) : null}
      {conn.readOnly && (
        <span role="img" aria-label="read-only" title="Read-only connection">
          <Lock className="size-3 shrink-0 text-dim" />
        </span>
      )}
    </span>
  )
}

/** Thin bar in the connection colour across the top of the editor. */
export function ConnectionStripe({ connectionId }: { connectionId: string }) {
  const conn = useStore((s) => s.connections[connectionId])
  const color = conn ? connectionColor(conn) : null
  if (!color) return null
  return <div aria-hidden className="h-0.5 shrink-0" style={{ background: color }} />
}

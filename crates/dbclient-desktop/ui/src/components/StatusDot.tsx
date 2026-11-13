// Status dot of the v3 design: filled for connected (green), connecting
// (amber), failed (red), busy (blue); a dashed ring when disconnected.
import React from 'react'
import { cn } from '@/lib/utils'
import type { ConnState } from '@/state/types'

export type DotKind = 'run' | 'wait' | 'err' | 'ok' | 'idle' | 'disc'

export const DOT_LABEL: Record<DotKind, string> = {
  run: 'Busy',
  wait: 'Connecting',
  err: 'Failed',
  ok: 'Connected',
  idle: 'Idle',
  disc: 'Disconnected'
}

const KIND: Record<DotKind, string> = {
  run: 'bg-status-run',
  wait: 'bg-status-wait',
  err: 'bg-status-err',
  ok: 'bg-status-ok',
  idle: 'border-[1.5px] border-dim',
  disc: 'border-[1.5px] border-dashed border-dim'
}

export function connDot(state: ConnState): DotKind {
  return state === 'connected'
    ? 'ok'
    : state === 'connecting'
      ? 'wait'
      : state === 'failed'
        ? 'err'
        : 'disc'
}

type Props = {
  kind: DotKind
  /** 6px (headers), 7px (rows, tabs; default) */
  size?: 6 | 7
  className?: string
  title?: string
}

export const StatusDot = React.memo(function StatusDot({
  kind,
  size = 7,
  className,
  title
}: Props) {
  return (
    <span
      className={cn(
        'inline-block shrink-0 rounded-full',
        size === 6 ? 'size-1.5' : 'size-[7px]',
        KIND[kind],
        className
      )}
      role="img"
      aria-label={title ?? DOT_LABEL[kind]}
      title={title ?? DOT_LABEL[kind]}
      data-status={kind}
    />
  )
})

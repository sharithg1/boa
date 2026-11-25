// In-app toasts, rendered by sonner (shadcn `sonner`, see ui/sonner.tsx). The
// store stays the source of truth (pushToast / dismissToast, TTL, keyed
// replacement and the 5-toast cap live there); this bridge mirrors
// store.toasts into sonner, updating a toast in place when it was replaced.
import { useEffect, useRef } from 'react'
import { X } from 'lucide-react'
import { toast as sonner } from 'sonner'
import { Toaster } from '@/components/ui/sonner'
import { dismissToast, useStore, type Toast } from '@/state/store'
import { cn } from '@/lib/utils'
import { StatusDot } from './StatusDot'

export function Toasts() {
  const toasts = useStore((s) => s.toasts)
  // What sonner shows for each id (a keyed toast can be replaced under the same id).
  const shown = useRef(new Map<number, Toast>())

  useEffect(() => {
    const live = new Set(toasts.map((t) => t.id))
    for (const id of shown.current.keys()) {
      if (!live.has(id)) {
        shown.current.delete(id)
        sonner.dismiss(id)
      }
    }
    for (const t of toasts) {
      if (shown.current.get(t.id) === t) continue
      shown.current.set(t.id, t)
      sonner.custom(() => <ToastCard toast={t} />, {
        id: t.id,
        // The store owns expiry (pushToast's ttlMs).
        duration: Infinity,
        onDismiss: () => dismissToast(t.id)
      })
    }
  }, [toasts])

  return <Toaster />
}

function ToastCard({ toast: t }: { toast: Toast }) {
  return (
    <div
      role={t.kind === 'error' ? 'alert' : 'status'}
      data-toast-kind={t.kind}
      className={cn(
        'group/toast ml-auto flex w-max max-w-[400px] items-start gap-2.5 rounded-lg border border-border bg-muted px-[13px] py-[9px] text-sm text-body shadow-floating'
      )}
    >
      {t.kind !== 'info' && (
        <StatusDot kind={t.kind === 'error' ? 'err' : 'ok'} className="mt-[5px]" />
      )}
      <div className="min-w-0 flex-1">
        <div className={cn(t.kind !== 'info' && 'font-medium text-foreground')}>{t.title}</div>
        {t.body && (
          <div className="mt-0.5 line-clamp-3 text-xs whitespace-pre-line text-muted-foreground">
            {t.body}
          </div>
        )}
      </div>
      {t.action && (
        <button
          type="button"
          className="-my-0.5 shrink-0 cursor-pointer rounded-[4px] px-1.5 py-0.5 text-sm font-medium text-foreground hover:bg-hover-strong"
          onClick={() => {
            t.action?.run()
            dismissToast(t.id)
          }}
        >
          {t.action.label}
        </button>
      )}
      {t.kind !== 'info' && (
        <button
          type="button"
          aria-label="Dismiss"
          className="-mr-1 grid size-4 shrink-0 cursor-pointer place-items-center rounded-[3px] text-dim opacity-0 group-hover/toast:opacity-100 hover:text-foreground focus-visible:opacity-100"
          onClick={() => dismissToast(t.id)}
        >
          <X className="size-3.5" />
        </button>
      )}
    </div>
  )
}

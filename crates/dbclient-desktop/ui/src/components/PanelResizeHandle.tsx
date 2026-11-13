// Drag handle on a side panel's inner edge (the sidebar's right edge, the
// right panel's left edge). The panel keeps the width while dragging and
// saves it once, on release; double-click restores the default.
import { useState, type PointerEvent } from 'react'
import { cn } from '@/lib/utils'

export function PanelResizeHandle({
  edge,
  label,
  width,
  clamp,
  onDrag,
  onCommit,
  onReset
}: {
  /** Which edge of the panel it sits on; dragging away from the panel grows it. */
  edge: 'left' | 'right' | 'top'
  label: string
  width: number
  clamp: (px: number) => number
  /** The width while dragging, null when the drag ends. */
  onDrag: (w: number | null) => void
  onCommit: (w: number) => void
  onReset: () => void
}) {
  const vertical = edge === 'top'
  const pos = (e: PointerEvent) => (vertical ? e.clientY : e.clientX)
  const [start, setStart] = useState<{ x: number; width: number } | null>(null)
  const at = (e: PointerEvent) => {
    const d = pos(e) - start!.x
    return clamp(start!.width + (edge === 'right' ? d : -d))
  }
  return (
    <div
      role="separator"
      aria-orientation={vertical ? 'horizontal' : 'vertical'}
      aria-label={label}
      aria-valuenow={width}
      title="Drag to resize, double-click to reset"
      onPointerDown={(e) => {
        if (e.button !== 0) return
        e.preventDefault()
        e.currentTarget.setPointerCapture(e.pointerId)
        setStart({ x: pos(e), width })
      }}
      onPointerMove={(e) => {
        if (start) onDrag(at(e))
      }}
      onPointerUp={(e) => {
        if (!start) return
        onCommit(at(e))
        setStart(null)
        onDrag(null)
      }}
      onPointerCancel={() => {
        setStart(null)
        onDrag(null)
      }}
      onDoubleClick={onReset}
      className={cn(
        'absolute z-10 transition-colors hover:bg-border-strong',
        vertical
          ? 'inset-x-0 -top-[3px] h-1.5 cursor-row-resize'
          : 'inset-y-0 w-1.5 cursor-col-resize',
        edge === 'right' && '-right-[3px]',
        edge === 'left' && '-left-[3px]',
        start && 'bg-border-strong'
      )}
    />
  )
}

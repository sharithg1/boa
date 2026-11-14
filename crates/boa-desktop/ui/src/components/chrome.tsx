// Small chrome primitives shared by the title bar, sidebar, palette and
// settings: coloured tile, keycap, and the drawn panel-toggle and search glyphs.
import { cn } from '@/lib/utils'
import { tileColor } from '@/lib/format'

/** Coloured square with a name's initial (sidebar 16px, title bar 14px, palette 10px). */
export function Tile({
  name,
  size = 16,
  className
}: {
  name: string
  size?: 10 | 12 | 14 | 16
  className?: string
}) {
  return (
    <span
      aria-hidden
      className={cn(
        'grid shrink-0 place-items-center font-semibold text-primary-foreground',
        size >= 14 ? 'rounded-sm' : 'rounded-[3px]',
        size === 16 && 'size-4 text-[10px]',
        size === 14 && 'size-3.5 text-[9px]',
        size === 12 && 'size-3',
        size === 10 && 'size-2.5',
        className
      )}
      style={{ background: tileColor(name) }}
    >
      {size >= 14 ? name.slice(0, 1).toUpperCase() : null}
    </span>
  )
}

/** Keycap hint (⌘K). */
export function Kbd({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <kbd
      className={cn(
        'rounded-sm border border-border bg-muted px-[5px] font-mono text-2xs leading-[17px] font-normal text-muted-foreground',
        className
      )}
    >
      {children}
    </kbd>
  )
}

/** The design's drawn panel glyph: a rounded rect with a filled bar on one side. */
export function PanelGlyph({ side }: { side: 'left' | 'right' }) {
  return (
    <span
      aria-hidden
      className={cn(
        'flex h-[11px] rounded-[2px] border-[1.5px] border-quiet',
        side === 'left' ? 'w-[13px]' : 'w-[14px] justify-end'
      )}
    >
      <span className="h-full w-1 bg-quiet" />
    </span>
  )
}

/** The design's search glyph: a small hollow circle. */
export function RingGlyph({ size = 10 }: { size?: 10 | 11 }) {
  return (
    <span
      aria-hidden
      className={cn(
        'shrink-0 rounded-full border-[1.5px] border-faint',
        size === 10 ? 'size-2.5' : 'size-[11px]'
      )}
    />
  )
}

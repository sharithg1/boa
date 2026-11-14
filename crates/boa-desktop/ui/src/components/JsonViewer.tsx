// Viewer for json / jsonb cells: a collapsible tree (objects and arrays fold,
// with a key / item count when folded) or the pretty-printed text, both
// coloured like the grid. Opened by double-clicking a JSON cell, pressing
// Enter on it, or the braces button that shows on hover.
import { memo, useMemo, useState } from 'react'
import { ChevronRight, Copy } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { cn } from '@/lib/utils'
import { pushToast } from '@/state/store'
import {
  JSON_CLASS,
  formatJson,
  jsonTokens,
  parseJson,
  type JsonNode,
  type JsonTokenKind
} from '@/lib/values'

export type JsonTarget = { column: string; type: string; row: number; value: string }

export function JsonViewer({
  target,
  onClose
}: {
  target: JsonTarget | null
  onClose: () => void
}) {
  return (
    <Dialog open={!!target} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="flex h-[min(80vh,720px)] flex-col gap-0 p-0 sm:max-w-3xl">
        {target && <Body target={target} />}
      </DialogContent>
    </Dialog>
  )
}

type Mode = 'tree' | 'text'

function Body({ target }: { target: JsonTarget }) {
  const [mode, setMode] = useState<Mode>('tree')
  // Remounting the tree under a new key applies expand / collapse all.
  const [fold, setFold] = useState<{ open: boolean | null; gen: number }>({ open: null, gen: 0 })
  const parsed = useMemo(() => {
    try {
      return { ok: true as const, node: parseJson(target.value) }
    } catch (e) {
      return { ok: false as const, error: (e as Error).message }
    }
  }, [target.value])
  const pretty = useMemo(() => formatJson(target.value), [target.value])

  const copy = () => {
    void navigator.clipboard?.writeText(pretty).catch(() => {})
    pushToast({ kind: 'info', title: 'Copied JSON' }, 1600)
  }

  return (
    <>
      <div className="flex items-center gap-3 border-b border-border py-3 pr-12 pl-4">
        <div className="min-w-0">
          <DialogTitle className="truncate font-mono text-sm font-medium">
            {target.column}
          </DialogTitle>
          <DialogDescription className="text-xs text-quiet">
            Row {target.row} · <span className="text-sql-type">{target.type}</span> ·{' '}
            {target.value.length.toLocaleString()} chars
          </DialogDescription>
        </div>
        <div className="ml-auto flex shrink-0 items-center gap-1">
          {mode === 'tree' && parsed.ok && (
            <>
              <Button
                variant="ghost"
                size="xs"
                onClick={() => setFold((f) => ({ open: true, gen: f.gen + 1 }))}
              >
                Expand all
              </Button>
              <Button
                variant="ghost"
                size="xs"
                onClick={() => setFold((f) => ({ open: false, gen: f.gen + 1 }))}
              >
                Collapse all
              </Button>
            </>
          )}
          <div className="ml-1 flex rounded-md border border-border p-0.5 text-xs">
            {(['tree', 'text'] as const).map((m) => (
              <button
                key={m}
                type="button"
                onClick={() => setMode(m)}
                className={cn(
                  'rounded-[4px] px-2 py-0.5 capitalize',
                  mode === m ? 'bg-secondary text-foreground' : 'text-quiet hover:text-soft'
                )}
              >
                {m}
              </button>
            ))}
          </div>
          <Button variant="ghost" size="xs" onClick={copy}>
            <Copy className="size-3.5" />
            Copy
          </Button>
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-auto px-4 py-3 font-mono text-sm select-text">
        {mode === 'tree' && parsed.ok ? (
          <Node key={fold.gen} node={parsed.node} depth={0} forceOpen={fold.open} last />
        ) : (
          <>
            {!parsed.ok && mode === 'tree' && (
              <div className="mb-3 text-xs text-status-err-text">
                Not valid JSON: {parsed.error}
              </div>
            )}
            <JsonText text={pretty} />
          </>
        )}
      </div>
    </>
  )
}

/** JSON text coloured token by token (used in grid cells and the text view). */
export const JsonText = memo(function JsonText({ text }: { text: string }) {
  return (
    <pre className="m-0 font-[inherit] whitespace-pre">
      {jsonTokens(text).map((t, i) => (
        <Tok key={i} kind={t.kind} text={t.text} />
      ))}
    </pre>
  )
})

/** Inline (single-line) JSON for a grid cell: only the visible head is tokenized. */
export const JsonInline = memo(function JsonInline({ text }: { text: string }) {
  const head = text.length > 400 ? text.slice(0, 400) : text
  return (
    <>
      {jsonTokens(head).map((t, i) => (
        <Tok key={i} kind={t.kind} text={t.text} />
      ))}
    </>
  )
})

function Tok({ kind, text }: { kind: JsonTokenKind; text: string }) {
  return kind === 'plain' ? text : <span className={JSON_CLASS[kind]}>{text}</span>
}

function Node({
  node,
  depth,
  name,
  forceOpen,
  last
}: {
  node: JsonNode
  depth: number
  name?: string | number
  forceOpen: boolean | null
  last: boolean
}) {
  const [open, setOpen] = useState(forceOpen ?? depth < 2)
  const comma = last ? null : <span className={JSON_CLASS.punct}>,</span>
  const label =
    name === undefined ? null : typeof name === 'number' ? (
      <span className="text-dim">{name}: </span>
    ) : (
      <>
        <span className={JSON_CLASS.key}>{JSON.stringify(name)}</span>
        <span className={JSON_CLASS.punct}>: </span>
      </>
    )

  if (node.t !== 'object' && node.t !== 'array') {
    return (
      <div className="pl-[18px] break-all whitespace-pre-wrap">
        {label}
        <span className={JSON_CLASS[node.t === 'string' ? 'string' : node.t]}>{node.src}</span>
        {comma}
      </div>
    )
  }

  const [o, c] = node.t === 'object' ? ['{', '}'] : ['[', ']']
  const children: [string | number, JsonNode][] =
    node.t === 'object' ? node.entries : node.items.map((v, i) => [i, v])
  if (children.length === 0) {
    return (
      <div className="pl-[18px]">
        {label}
        <span className={JSON_CLASS.punct}>
          {o}
          {c}
        </span>
        {comma}
      </div>
    )
  }
  const count = `${children.length} ${node.t === 'object' ? 'key' : 'item'}${children.length === 1 ? '' : 's'}`
  return (
    <div>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center text-left hover:bg-hover"
      >
        <ChevronRight
          className={cn('size-3.5 shrink-0 text-dim transition-transform', open && 'rotate-90')}
        />
        <span className="ml-[4px]">
          {label}
          <span className={JSON_CLASS.punct}>{o}</span>
          {!open && (
            <>
              <span className="mx-1 text-xs text-dim italic">{count}</span>
              <span className={JSON_CLASS.punct}>{c}</span>
              {comma}
            </>
          )}
        </span>
      </button>
      {open && (
        <>
          <div className="ml-[7px] border-l border-border-subtle pl-[11px]">
            {children.map(([k, v], i) => (
              <Node
                key={k}
                node={v}
                depth={depth + 1}
                name={k}
                forceOpen={forceOpen}
                last={i === children.length - 1}
              />
            ))}
          </div>
          <div className="pl-[18px]">
            <span className={JSON_CLASS.punct}>{c}</span>
            {comma}
          </div>
        </>
      )}
    </div>
  )
}

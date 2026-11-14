// Larger editor for one cell of the data editor: long or multi-line text and
// JSON (with Format). Opened with
// ⇧↵, or by editing a multi-line / JSON / long value. ⌘↵ applies, Esc
// cancels, "Set NULL" stores NULL (distinct from an empty string).
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { formatJson } from '@/lib/values'
import { Kbd } from './chrome'

export type ValueTarget = {
  column: string
  type: string
  row: number
  value: string | null
  json: boolean
}

export function ValueEditor({
  target,
  onApply,
  onClose
}: {
  target: ValueTarget | null
  onApply: (value: string | null) => void
  onClose: () => void
}) {
  return (
    <Dialog open={!!target} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="flex h-[min(70vh,560px)] flex-col gap-0 p-0 sm:max-w-2xl">
        {target && <Body target={target} onApply={onApply} />}
      </DialogContent>
    </Dialog>
  )
}

function Body({
  target,
  onApply
}: {
  target: ValueTarget
  onApply: (value: string | null) => void
}) {
  const [text, setText] = useState(target.value ?? '')
  // NULL until the user types (an emptied field means '').
  const [isNull, setIsNull] = useState(target.value === null)
  const [touched, setTouched] = useState(false)
  const apply = () => (touched ? onApply(isNull ? null : text) : onApply(target.value))

  return (
    <>
      <div className="flex items-center gap-3 border-b border-border py-3 pr-12 pl-4">
        <div className="min-w-0">
          <DialogTitle className="truncate font-mono text-sm font-medium">
            {target.column}
          </DialogTitle>
          <DialogDescription className="text-xs text-quiet">
            Row {target.row} · <span className="text-sql-type">{target.type}</span>
            {isNull ? ' · NULL' : ` · ${text.length.toLocaleString()} chars`}
          </DialogDescription>
        </div>
        <div className="ml-auto flex shrink-0 items-center gap-1">
          {target.json && (
            <Button
              variant="ghost"
              size="xs"
              disabled={isNull}
              onClick={() => {
                setText(formatJson(text))
                setTouched(true)
              }}
            >
              Format
            </Button>
          )}
          <Button
            variant="ghost"
            size="xs"
            disabled={isNull}
            onClick={() => {
              setIsNull(true)
              setTouched(true)
            }}
          >
            Set NULL
          </Button>
        </div>
      </div>
      <textarea
        autoFocus
        spellCheck={false}
        value={text}
        placeholder={isNull ? '<null>' : ''}
        onChange={(e) => {
          setText(e.target.value)
          setIsNull(false)
          setTouched(true)
        }}
        onKeyDown={(e) => {
          if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
            e.preventDefault()
            apply()
          }
        }}
        className="min-h-0 flex-1 resize-none bg-editor px-4 py-3 font-mono text-sm text-body outline-none placeholder:text-dim placeholder:italic"
      />
      <div className="flex items-center gap-2 border-t border-border px-4 py-2.5">
        <span className="flex items-center gap-1.5 text-xs text-dim">
          <Kbd>⌘↵</Kbd> apply · <Kbd>Esc</Kbd> cancel
        </span>
        <Button size="xs" className="ml-auto" onClick={apply}>
          Apply
        </Button>
      </div>
    </>
  )
}

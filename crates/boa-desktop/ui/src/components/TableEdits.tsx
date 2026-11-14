// Data editing for table tabs (DataGrip's data editor): `useTableEdits`
// joins a tab's loaded page with its pending edits (state/edits.ts) into the
// rows and `GridEditing` hooks the DataGrid shows, plus the toolbar controls
// (add / delete row, revert, change count, preview, submit) or the reason
// the table is read-only. Changes are only staged until Submit, which runs
// the statements shown in the preview in one transaction.
import { useEffect, useMemo, useState, type ReactNode } from 'react'
import { Copy, FileCode, Lock, Minus, Plus, Undo2 } from 'lucide-react'
import { tokenize } from '@/lib/sql'
import { cn } from '@/lib/utils'
import type { QueryResult } from '@/lib/backend'
import {
  addRow,
  buildStatements,
  cellEdited,
  changeCount,
  deleteRow,
  editability,
  EMPTY_PENDING,
  revertCell,
  revertRow,
  rowMark,
  setCell,
  type EditStatement,
  type Pending,
  type RowRef,
  type Value
} from '@/lib/gridEdits'
import {
  clearEdits,
  confirmDiscard,
  submitEdits,
  updatePending,
  useEdits,
  type SubmitError
} from '@/state/edits'
import { newQueryTab, pushToast } from '@/state/store'
import type { Table } from '@/state/types'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle
} from '@/components/ui/alert-dialog'
import { Spinner } from '@/components/ui/spinner'
import { Kbd } from './chrome'
import type { GridEditing } from './DataGrid'
import { IconButton } from './IconButton'
import { CLASS } from './SqlHighlight'

/** The page a tab's pending edits refer to, to show again instead of reloading. */
export const cachedEditPage = (tabId: string) => useEdits.getState().tabs[tabId]

export function useTableEdits({
  tabId,
  connectionId,
  database,
  schema,
  table,
  result,
  offset,
  rowLimit,
  filter,
  onSubmitted
}: {
  tabId: string
  connectionId: string
  database: string
  schema: string
  table: Table | undefined
  result: QueryResult | null
  offset: number
  rowLimit: number
  filter: string
  /** Reload the page after a successful submit. */
  onSubmitted: () => void
}) {
  const entry = useEdits((s) => s.tabs[tabId])
  const submitting = useEdits((s) => !!s.submitting[tabId])
  const error = useEdits((s) => s.errors[tabId])
  const [preview, setPreview] = useState(false)
  const [selRow, setSelRow] = useState<number | null>(null)
  // Edits made on an older page don't apply to this one.
  const pending: Pending = entry && entry.result === result ? entry.pending : EMPTY_PENDING
  const columns = result?.columns ?? []
  const can = editability(table, columns)
  const count = changeCount(pending)

  // The page was replaced under the edits (e.g. a new row limit): they no longer apply.
  useEffect(() => {
    if (!entry || !result || entry.result === result) return
    clearEdits(tabId)
    pushToast({ kind: 'info', title: 'Pending changes discarded', body: 'The rows were reloaded.' })
  }, [entry, result, tabId])

  const { rows, refs } = useMemo(() => {
    const base = result?.rows ?? []
    const q = filter.trim().toLowerCase()
    const rows: Value[][] = []
    const refs: RowRef[] = []
    base.forEach((r, index) => {
      const upd = pending.updates[index]
      const row = upd ? r.map((v, c) => (c in upd ? upd[c]! : v)) : r
      if (q && !row.some((v) => v?.toLowerCase().includes(q))) return
      rows.push(row)
      refs.push({ kind: 'base', index })
    })
    for (const n of pending.inserts) {
      rows.push(columns.map((_, c) => n.values[c] ?? null))
      refs.push({ kind: 'new', id: n.id })
    }
    return { rows, refs }
  }, [result, pending, filter, columns])

  const target = can.editable
    ? { schema, table: table!.name, columns: columns.map((c) => c.name), pk: can.pk }
    : null
  const statements: EditStatement[] =
    target && result && preview ? buildStatements(target, result.rows, pending) : []

  const update = (f: (p: Pending) => Pending) => {
    if (result) updatePending(tabId, { result, offset, rowLimit }, f)
  }
  const defaults = useMemo(
    () => columns.map((c) => !!table?.columns.find((t) => t.name === c.name)?.default),
    [columns, table]
  )

  const editing: GridEditing | undefined =
    can.editable && result
      ? {
          rowMark: (r) => (refs[r] ? rowMark(pending, refs[r]) : undefined),
          cellEdited: (r, c) => !!refs[r] && cellEdited(pending, refs[r], c),
          placeholder: (r, c) =>
            refs[r]?.kind === 'new' && defaults[c] && !cellEdited(pending, refs[r], c)
              ? '<default>'
              : undefined,
          set: (r, c, v) => refs[r] && update((p) => setCell(p, result.rows, refs[r]!, c, v)),
          revertCell: (r, c) => refs[r] && update((p) => revertCell(p, refs[r]!, c)),
          revertRow: (r) => refs[r] && update((p) => revertRow(p, refs[r]!)),
          deleteRow: (r) => refs[r] && update((p) => deleteRow(p, refs[r]!)),
          addRow: () => update((p) => addRow(p).pending),
          newRows: pending.inserts.length,
          submit: () => count && setPreview(true),
          onSelectRow: setSelRow
        }
      : undefined

  /** Runs `go` now, or after confirming that pending edits will be lost. */
  const guard = (action: string, go: () => void) => {
    if (!count) {
      if (entry) clearEdits(tabId)
      go()
      return
    }
    void confirmDiscard(tabId, action).then((ok) => ok && go())
  }

  const submit = async () => {
    if (!target) return
    if (await submitEdits(tabId, connectionId, database, target)) {
      setPreview(false)
      onSubmitted()
    } else setPreview(true) // shows the failing statement
  }

  const selRef = selRow != null ? refs[selRow] : undefined
  const toolbar: ReactNode = !result ? null : !can.editable ? (
    <span className="flex items-center gap-1 px-1.5 text-xs text-dim" title={can.reason}>
      <Lock className="size-3" />
      {can.reason}
    </span>
  ) : (
    <>
      <IconButton label="Add row (⌘N)" side="bottom" onClick={() => editing?.addRow()}>
        <Plus className="size-3.5" />
      </IconButton>
      <IconButton
        label="Delete row (⌘⌫)"
        side="bottom"
        disabled={!selRef || rowMark(pending, selRef) === 'deleted'}
        onClick={() => selRef && update((p) => deleteRow(p, selRef))}
      >
        <Minus className="size-3.5" />
      </IconButton>
      <IconButton
        label="Revert all changes"
        side="bottom"
        disabled={!count || submitting}
        onClick={() => clearEdits(tabId)}
      >
        <Undo2 className="size-3.5" />
      </IconButton>
      {count > 0 && (
        <>
          <span className="shrink-0 px-1 text-xs text-status-run" title="Pending changes">
            {count} change{count === 1 ? '' : 's'}
          </span>
          <Button variant="ghost" size="xs" onClick={() => setPreview(true)}>
            Preview…
          </Button>
          <Button size="xs" disabled={submitting} onClick={() => void submit()}>
            {submitting && <Spinner className="size-3" />}
            Submit
          </Button>
        </>
      )}
    </>
  )

  const dialogs = (
    <PreviewDialog
      open={preview && count > 0}
      title={`${schema}.${table?.name ?? ''}`}
      statements={statements}
      error={error}
      submitting={submitting}
      onClose={() => setPreview(false)}
      onSubmit={() => void submit()}
      onConsole={() => {
        setPreview(false)
        newQueryTab(connectionId, database, statementsText(statements))
      }}
    />
  )

  return { rows, editing, toolbar, dialogs, guard, pendingCount: count }
}

const statementsText = (sts: EditStatement[]) => sts.map((s) => `${s.sql};`).join('\n')

function SqlLine({ sql }: { sql: string }) {
  return (
    <>
      {tokenize(sql).map((t, i) =>
        t.kind === 'plain' ? (
          t.text
        ) : (
          <span key={i} className={CLASS[t.kind]}>
            {t.text}
          </span>
        )
      )}
      ;
    </>
  )
}

function PreviewDialog({
  open,
  title,
  statements,
  error,
  submitting,
  onClose,
  onSubmit,
  onConsole
}: {
  open: boolean
  title: string
  statements: EditStatement[]
  error: SubmitError | undefined
  submitting: boolean
  onClose: () => void
  onSubmit: () => void
  onConsole: () => void
}) {
  const n = statements.length
  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="flex max-h-[min(80vh,720px)] flex-col gap-0 p-0 sm:max-w-3xl">
        <div className="border-b border-border py-3 pr-12 pl-4">
          <DialogTitle className="text-sm font-medium">
            Submit changes to <span className="font-mono">{title}</span>
          </DialogTitle>
          <DialogDescription className="text-xs text-quiet">
            {n} statement{n === 1 ? '' : 's'}, run in one transaction (a savepoint inside an open
            one). Each UPDATE and DELETE must affect exactly one row; any error rolls back all of
            them.
          </DialogDescription>
        </div>
        <ol className="min-h-0 flex-1 overflow-auto bg-editor py-2 font-mono text-sm select-text">
          {statements.map((s, i) => {
            const failed = error?.index === i
            return (
              <li key={i} className={cn('flex gap-3 px-4 py-0.5', failed && 'bg-status-err/15')}>
                <span className="w-6 shrink-0 text-right text-dim select-none">{i + 1}</span>
                <div className="min-w-0 flex-1">
                  <pre className="m-0 whitespace-pre-wrap break-all text-body">
                    <SqlLine sql={s.sql} />
                  </pre>
                  {failed && (
                    <div className="mt-0.5 font-sans text-xs text-status-err-text whitespace-pre-wrap">
                      {error.message}
                      {error.detail ? `\n${error.detail}` : ''}
                    </div>
                  )}
                </div>
              </li>
            )
          })}
        </ol>
        {error && error.index < 0 && (
          <div className="border-t border-border px-4 py-2 text-xs text-status-err-text whitespace-pre-wrap">
            {error.message}
          </div>
        )}
        <div className="flex items-center gap-2 border-t border-border px-4 py-2.5">
          <Button
            variant="ghost"
            size="xs"
            onClick={() => {
              void navigator.clipboard?.writeText(statementsText(statements)).catch(() => {})
              pushToast({ kind: 'info', title: 'Copied SQL' }, 1600)
            }}
          >
            <Copy />
            Copy
          </Button>
          <Button variant="ghost" size="xs" onClick={onConsole}>
            <FileCode />
            Open in console
          </Button>
          <span className="ml-auto flex items-center gap-1.5 text-xs text-dim">
            <Kbd>↵</Kbd> submit
          </span>
          <Button variant="outline" size="xs" onClick={onClose}>
            Cancel
          </Button>
          <Button size="xs" autoFocus disabled={submitting || !n} onClick={onSubmit}>
            {submitting && <Spinner className="size-3" />}
            Submit
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  )
}

/** "Discard pending changes?" before reloading, paging or closing a table tab. Mounted once. */
export function DiscardEditsDialog() {
  const prompt = useEdits((s) => s.discard)
  return (
    <AlertDialog open={!!prompt} onOpenChange={(o) => !o && prompt?.resolve(false)}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>
            Discard {prompt?.count} pending change{prompt?.count === 1 ? '' : 's'}?
          </AlertDialogTitle>
          <AlertDialogDescription>
            They have not been submitted. To {prompt?.action}, they will be lost.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel onClick={() => prompt?.resolve(false)}>Keep editing</AlertDialogCancel>
          <AlertDialogAction variant="destructive" onClick={() => prompt?.resolve(true)}>
            Discard
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

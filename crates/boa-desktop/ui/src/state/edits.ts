// Pending grid edits of table tabs, by tab id, with the page they apply to
// (so switching tabs doesn't reload the rows under them). Nothing reaches
// the database until `submitEdits`, which runs the generated statements as
// one unit through `backend.applyEdits`. Reloading, paging or closing a tab
// with pending edits asks first (`confirmDiscard`, the close guard).
import { create } from 'zustand'
import { backend, toBackendError, type QueryResult } from '@/lib/backend'
import {
  EMPTY_PENDING,
  buildStatements,
  changeCount,
  isEmpty,
  type EditTarget,
  type Pending
} from '@/lib/gridEdits'
import { closeTab, logRun, pushToast, setCloseGuard, useStore } from './store'

export type TabEdits = {
  pending: Pending
  /** The loaded page the edits refer to (row indices are into its rows). */
  result: QueryResult
  offset: number
  rowLimit: number
}

export type DiscardPrompt = {
  /** What happens after discarding: "reload", "close the tab"… */
  action: string
  count: number
  resolve: (ok: boolean) => void
}

/** Failure of the last submit, shown in the preview. */
export type SubmitError = { index: number; message: string; detail?: string }

type EditsState = {
  tabs: Record<string, TabEdits>
  submitting: Record<string, true>
  errors: Record<string, SubmitError>
  discard: DiscardPrompt | null
}

export const useEdits = create<EditsState>(() => ({
  tabs: {},
  submitting: {},
  errors: {},
  discard: null
}))

const without = <T>(o: Record<string, T>, k: string) => {
  const { [k]: _, ...rest } = o
  return rest
}

/** Applies `f` to the tab's pending edits of `page` (starting over if the page changed). */
export function updatePending(
  tabId: string,
  page: Omit<TabEdits, 'pending'>,
  f: (p: Pending) => Pending
) {
  useEdits.setState((s) => {
    const cur = s.tabs[tabId]
    const pending = f(cur && cur.result === page.result ? cur.pending : EMPTY_PENDING)
    const errors = without(s.errors, tabId)
    if (isEmpty(pending)) return { tabs: without(s.tabs, tabId), errors }
    return { tabs: { ...s.tabs, [tabId]: { ...page, pending } }, errors }
  })
}

export function clearEdits(tabId: string) {
  useEdits.setState((s) => ({ tabs: without(s.tabs, tabId), errors: without(s.errors, tabId) }))
}

export function pendingCount(tabId: string): number {
  const t = useEdits.getState().tabs[tabId]
  return t ? changeCount(t.pending) : 0
}

/** Asks before throwing away the tab's pending edits; resolves true (and clears them) to go on. */
export function confirmDiscard(tabId: string, action: string): Promise<boolean> {
  const count = pendingCount(tabId)
  if (!count) {
    clearEdits(tabId)
    return Promise.resolve(true)
  }
  useEdits.getState().discard?.resolve(false)
  return new Promise((resolve) => {
    let done = false
    const prompt: DiscardPrompt = {
      action,
      count,
      // Called by both the button and the dialog closing: only the first counts.
      resolve: (ok) => {
        if (done) return
        done = true
        useEdits.setState((s) => (s.discard === prompt ? { discard: null } : {}))
        if (ok) clearEdits(tabId)
        resolve(ok)
      }
    }
    useEdits.setState({ discard: prompt })
  })
}

setCloseGuard((key, id) => {
  if (!useEdits.getState().tabs[id]) return true
  if (!pendingCount(id)) {
    clearEdits(id)
    return true
  }
  void confirmDiscard(id, 'close the tab').then((ok) => ok && closeTab(key, id, { force: true }))
  return false
})

function errorText(e: unknown): { message: string; detail?: string } {
  const err = toBackendError(e)
  const detail = [err.detail && `Detail: ${err.detail}`, err.hint && `Hint: ${err.hint}`]
    .filter(Boolean)
    .join('\n')
  return {
    message:
      err.code && /^[0-9A-Z]{5}$/.test(err.code) ? `[${err.code}] ${err.message}` : err.message,
    detail: detail || undefined
  }
}

/**
 * Submits the tab's pending edits. On success they are cleared (the caller
 * reloads the page); on failure everything was rolled back, the edits stay
 * and the failing statement is remembered for the preview.
 */
export async function submitEdits(
  tabId: string,
  connectionId: string,
  database: string,
  target: EditTarget
): Promise<boolean> {
  const s = useEdits.getState()
  const entry = s.tabs[tabId]
  if (!entry || s.submitting[tabId]) return false
  const statements = buildStatements(target, entry.result.rows, entry.pending)
  if (!statements.length) {
    clearEdits(tabId)
    return true
  }
  const source = target.table
  const at = Date.now()
  useEdits.setState((st) => ({ submitting: { ...st.submitting, [tabId]: true } }))
  try {
    const out = await backend.applyEdits(connectionId, database, statements)
    if (out.failed) {
      const { index, error } = out.failed
      const err = errorText(error)
      const note = `Statement ${index + 1} of ${statements.length} failed; all changes were rolled back.`
      logRun(connectionId, {
        sql: statements[index]?.sql ?? '',
        database,
        at,
        ms: out.ms,
        rows: null,
        source,
        error: err.message,
        detail: [note, err.detail].filter(Boolean).join('\n')
      })
      useEdits.setState((st) => ({
        errors: { ...st.errors, [tabId]: { index, ...err } }
      }))
      useStore.setState({ bottomOpen: true, bottomView: 'output' })
      pushToast({ kind: 'error', title: 'Changes not submitted', body: `${note}\n${err.message}` })
      return false
    }
    for (const [i, st] of statements.entries())
      logRun(connectionId, {
        sql: st.sql,
        database,
        at,
        ms: out.runs[i]?.ms ?? 0,
        rows: null,
        affected: out.runs[i]?.affected ?? 0,
        source
      })
    clearEdits(tabId)
    pushToast(
      out.inTransaction
        ? {
            kind: 'info',
            title: `${statements.length} change${statements.length === 1 ? '' : 's'} applied in the open transaction`,
            body: 'Commit the transaction to keep them.'
          }
        : {
            kind: 'success',
            title: `Submitted ${statements.length} change${statements.length === 1 ? '' : 's'}`
          },
      3000
    )
    return true
  } catch (e) {
    const err = errorText(e)
    logRun(connectionId, {
      sql: statements.map((x) => `${x.sql};`).join('\n'),
      database,
      at,
      ms: Date.now() - at,
      rows: null,
      source,
      error: err.message,
      detail: err.detail
    })
    useEdits.setState((st) => ({ errors: { ...st.errors, [tabId]: { index: -1, ...err } } }))
    pushToast({ kind: 'error', title: 'Changes not submitted', body: err.message })
    return false
  } finally {
    useEdits.setState((st) => ({ submitting: without(st.submitting, tabId) }))
  }
}

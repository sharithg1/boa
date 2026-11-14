// CSV export and "Copy as CSV". Read-only (pageable) statements and tables
// are exported whole by the backend, which streams a server-side COPY to the
// chosen file; results of statements that change data (`INSERT … RETURNING`)
// are never re-run: their loaded rows are written by lib/csv.ts. Running
// exports live here (shown in the status bar and the result toolbar), and
// the delimiter / header options persist in localStorage.

import { create } from 'zustand'
import { backend, toBackendError } from '@/lib/backend'
import {
  type CsvDelimiter,
  type CsvOptions,
  csvFileName,
  DEFAULT_CSV_OPTIONS,
  toCsv
} from '@/lib/csv'
import { bytes } from '@/lib/format'
import { qualified } from '@/lib/sql'
import { describeError, logOutput, pushToast, tableKey, useStore } from './store'
import type { ResultSet } from './types'

/** What to export: a statement and, when there is one, the rows already loaded for it. */
export type ExportSource = {
  connectionId: string
  database: string
  sql: string
  /** Output log source and fallback file name (`console_1`). */
  label: string
  /** Read-only: the backend may re-run it for the whole result. */
  pageable: boolean
  columns?: string[]
  rows?: (string | null)[][]
  /** More rows exist than were loaded. */
  truncated?: boolean
}

export type ExportJob = {
  id: number
  /** Who started it: a result's tab id or a table key, to show it as running there. */
  owner: string
  connectionId: string
  database: string
  file: string
  rows: number
  bytes: number
}

const KEY = 'boa.csvExport'

function loadOptions(): CsvOptions {
  try {
    const raw = typeof localStorage !== 'undefined' ? localStorage.getItem(KEY) : null
    const o = raw ? (JSON.parse(raw) as Partial<CsvOptions>) : {}
    return {
      delimiter: [',', '\t', ';'].includes(o.delimiter as string)
        ? (o.delimiter as CsvDelimiter)
        : DEFAULT_CSV_OPTIONS.delimiter,
      header: typeof o.header === 'boolean' ? o.header : DEFAULT_CSV_OPTIONS.header
    }
  } catch {
    return DEFAULT_CSV_OPTIONS
  }
}

export const useCsvExport = create<{ options: CsvOptions; jobs: ExportJob[] }>(() => ({
  options: loadOptions(),
  jobs: []
}))

export function setCsvOptions(patch: Partial<CsvOptions>) {
  useCsvExport.setState((s) => ({ options: { ...s.options, ...patch } }))
  try {
    localStorage.setItem(KEY, JSON.stringify(useCsvExport.getState().options))
  } catch {
    // Storage unavailable: keep the in-memory value.
  }
}

let nextJob = 1

function patchJob(id: number, patch: Partial<ExportJob>) {
  useCsvExport.setState((s) => ({
    jobs: s.jobs.map((j) => (j.id === id ? { ...j, ...patch } : j))
  }))
}

const plural = (n: number, w: string) => `${n.toLocaleString()} ${w}${n === 1 ? '' : 's'}`

const fileName = (path: string) => path.split(/[/\\]/).pop() ?? path

function done(path: string, title: string, body: string) {
  pushToast(
    {
      kind: 'success',
      title,
      body,
      action: backend.real
        ? {
            label: 'Reveal in Finder',
            run: () => void backend.revealPath(path).catch(() => {})
          }
        : undefined
    },
    10_000
  )
}

/**
 * Asks for a file and exports `src` to it: the whole result through the
 * backend when the statement is read-only, else the loaded rows.
 */
export async function exportCsv(src: ExportSource, owner: string) {
  const options = useCsvExport.getState().options
  const conn = useStore.getState().connections[src.connectionId]
  if (src.pageable && conn?.state !== 'connected') {
    pushToast({
      kind: 'error',
      title: 'Export failed',
      body: `${conn?.name ?? 'Connection'} is not connected`
    })
    return
  }
  let path: string | null
  try {
    path = await backend.chooseSavePath(csvFileName(src.sql, src.label, options.delimiter))
  } catch (e) {
    pushToast({ kind: 'error', title: 'Export failed', body: toBackendError(e).message })
    return
  }
  if (!path) return
  const at = Date.now()
  const base = { sql: src.sql, database: src.database, at, source: src.label }

  if (!src.pageable) {
    // Changes data: never re-run, write what was loaded.
    const rows = src.rows ?? []
    try {
      await backend.writeCsvFile(path, toCsv(src.columns ?? [], rows, options))
      logOutput(src.connectionId, {
        ...base,
        ms: Date.now() - at,
        rows: null,
        note: `${plural(rows.length, 'loaded row')} exported to ${path} (not re-run)`
      })
      done(
        path,
        `Exported ${plural(rows.length, 'row')}`,
        src.truncated
          ? `Only the ${plural(rows.length, 'loaded row')}: the statement changes data, so it was not re-run for the rest.\n${path}`
          : `The loaded result (the statement changes data, so it was not re-run).\n${path}`
      )
    } catch (e) {
      failed(src, base, e)
    }
    return
  }

  const id = nextJob++
  useCsvExport.setState((s) => ({
    jobs: [
      ...s.jobs,
      {
        id,
        owner,
        connectionId: src.connectionId,
        database: src.database,
        file: fileName(path),
        rows: 0,
        bytes: 0
      }
    ]
  }))
  try {
    const res = await backend.exportCsv(
      src.connectionId,
      src.database,
      src.sql,
      path,
      options,
      (p) => patchJob(id, p)
    )
    logOutput(src.connectionId, {
      ...base,
      ms: res.ms,
      rows: null,
      note: `${plural(res.rows, 'row')} exported to ${path} (${bytes(res.bytes)}) in ${res.ms} ms`
    })
    done(path, `Exported ${plural(res.rows, 'row')}`, `${path} · ${bytes(res.bytes)}`)
  } catch (e) {
    failed(src, base, e)
  } finally {
    useCsvExport.setState((s) => ({ jobs: s.jobs.filter((j) => j.id !== id) }))
  }
}

function failed(
  src: ExportSource,
  base: { sql: string; database: string; at: number; source: string },
  e: unknown
) {
  const err = describeError(e)
  const cancelled = toBackendError(e).code === '57014'
  logOutput(src.connectionId, {
    ...base,
    ms: Date.now() - base.at,
    rows: null,
    ...(cancelled
      ? { note: 'Export cancelled' }
      : { error: `Export failed: ${err.error}`, detail: err.detail })
  })
  if (cancelled) pushToast({ kind: 'info', title: 'Export cancelled' })
  else pushToast({ kind: 'error', title: 'Export failed', body: err.error })
}

/** Cancels a running export (a server-side cancel of its database's session). */
export function cancelExport(id: number) {
  const job = useCsvExport.getState().jobs.find((j) => j.id === id)
  if (!job) return
  void backend
    .cancelQuery(job.connectionId, job.database)
    .catch((e) =>
      pushToast({ kind: 'error', title: 'Cancel failed', body: toBackendError(e).message })
    )
}

export function resultSource(r: ResultSet): ExportSource {
  return {
    connectionId: r.connectionId,
    database: r.database,
    sql: r.sql,
    label: r.title,
    pageable: r.pageable,
    columns: r.columns.map((c) => c.name),
    rows: r.rows,
    truncated: !!r.truncated
  }
}

export function tableSource(
  connectionId: string,
  database: string,
  schema: string,
  table: string
): ExportSource {
  return {
    connectionId,
    database,
    sql: `select * from ${qualified(schema, table)}`,
    label: table,
    pageable: true
  }
}

/** Running-export owner of a table: shared by its explorer menu and its tab. */
export const tableOwner = tableKey

/** Exports a whole table or view (explorer menu); runs as `tableKey`, like its table tab. */
export function exportTable(connectionId: string, database: string, schema: string, table: string) {
  return exportCsv(
    tableSource(connectionId, database, schema, table),
    tableOwner(connectionId, database, schema, table)
  )
}

/** Copies the loaded rows (the page shown) to the clipboard as CSV. */
export async function copyCsv(columns: string[], rows: (string | null)[][]) {
  const text = toCsv(columns, rows, useCsvExport.getState().options)
  try {
    await navigator.clipboard.writeText(text)
    pushToast({ kind: 'info', title: `Copied ${plural(rows.length, 'row')} as CSV` })
  } catch (e) {
    pushToast({ kind: 'error', title: 'Copy failed', body: toBackendError(e).message })
  }
}

// CSV export calls of the backend (lib/backend.ts spreads these in): the
// Tauri commands `export_csv` (server-side COPY streamed to a file, with
// `csv-export-progress` events), `write_csv_file`, `reveal_path` and the
// native save dialog; in a plain browser, a mock that builds the CSV from
// the sample rows and downloads it.

import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { save } from '@tauri-apps/plugin-dialog'
import { type CsvOptions, toCsv } from '@/lib/csv'
import { execute } from '@/state/mockData'
import type { Database } from '@/state/types'

export type ExportProgress = { rows: number; bytes: number }

export type ExportResult = ExportProgress & { ms: number }

export type ExportBackend = {
  /** Asks where to save (native dialog); null when cancelled. The mock answers `defaultName`. */
  chooseSavePath(defaultName: string): Promise<string | null>
  /**
   * Streams the whole result of a read-only statement to `path` as CSV
   * (server-side COPY). Cancelled with `cancelQuery` on the same database.
   */
  exportCsv(
    id: string,
    database: string,
    sql: string,
    path: string,
    options: CsvOptions,
    onProgress?: (p: ExportProgress) => void
  ): Promise<ExportResult>
  /** Saves CSV built on the UI side (the mock downloads it). */
  writeCsvFile(path: string, contents: string): Promise<void>
  /** Shows the file in Finder. */
  revealPath(path: string): Promise<void>
}

let nextExport = 1

export const tauriExport: ExportBackend = {
  chooseSavePath: async (defaultName) =>
    (await save({
      defaultPath: defaultName,
      filters: [{ name: 'CSV', extensions: ['csv', 'tsv', 'txt'] }]
    })) ?? null,
  exportCsv: async (id, database, sql, path, options, onProgress) => {
    const exportId = `e${nextExport++}`
    const unlisten = onProgress
      ? await listen<ExportProgress & { exportId: string }>('csv-export-progress', (e) => {
          if (e.payload.exportId === exportId) onProgress(e.payload)
        })
      : undefined
    try {
      return await invoke<ExportResult>('export_csv', {
        id,
        database,
        sql,
        path,
        options,
        exportId
      })
    } finally {
      unlisten?.()
    }
  },
  writeCsvFile: (path, contents) => invoke('write_csv_file', { path, contents }),
  revealPath: (path) => invoke('reveal_path', { path })
}

/** Rows a mock export writes at most (sample tables claim millions). */
const MOCK_EXPORT_ROWS = 20_000

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

function download(name: string, contents: string) {
  const url = URL.createObjectURL(new Blob([contents], { type: 'text/csv;charset=utf-8' }))
  const a = document.createElement('a')
  a.href = url
  a.download = name.split(/[/\\]/).pop() || 'export.csv'
  document.body.append(a)
  a.click()
  a.remove()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}

export function mockExport(
  dbOf: (id: string, database: string) => Database | undefined,
  cancelled: Set<string>
): ExportBackend {
  return {
    chooseSavePath: async (defaultName) => defaultName,
    exportCsv: async (id, database, sql, path, options, onProgress) => {
      const key = `${id}\u0000${database}`
      cancelled.delete(key)
      const started = Date.now()
      const res = execute(dbOf(id, database), sql, MOCK_EXPORT_ROWS)
      if (res.kind === 'error') throw { message: res.message, code: '42P01' }
      if (res.kind === 'affected')
        throw { message: 'Only queries can be exported; this statement modifies data' }
      const csv = toCsv(
        res.columns.map((c) => c.name),
        res.rows,
        options
      )
      // A little simulated streaming, so the running state and Cancel show.
      const steps = 6
      for (let i = 1; i <= steps; i++) {
        await sleep(150)
        if (cancelled.has(key))
          throw { message: 'canceling statement due to user request', code: '57014' }
        onProgress?.({
          rows: Math.round((res.rows.length * i) / steps),
          bytes: Math.round((csv.length * i) / steps)
        })
      }
      download(path, csv)
      return { rows: res.rows.length, bytes: new Blob([csv]).size, ms: Date.now() - started }
    },
    writeCsvFile: async (path, contents) => download(path, contents),
    revealPath: async () => {}
  }
}

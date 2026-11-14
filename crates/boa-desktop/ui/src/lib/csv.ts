// CSV writer for results built on the UI side (rows already loaded: a page
// copied to the clipboard, or a result that must not be re-run). It writes
// what Postgres's `COPY … TO STDOUT WITH (FORMAT csv)` writes for the same
// rows, so both export paths give identical files: fields quoted only when
// they hold the delimiter, a quote, CR or LF (quotes doubled, RFC 4180), NULL
// as an empty unquoted field, an empty string as `""`, `\n` line ends.

export type CsvDelimiter = ',' | '\t' | ';'

export type CsvOptions = { delimiter: CsvDelimiter; header: boolean }

export const DEFAULT_CSV_OPTIONS: CsvOptions = { delimiter: ',', header: true }

export const CSV_DELIMITERS: { value: CsvDelimiter; label: string }[] = [
  { value: ',', label: 'Comma' },
  { value: '\t', label: 'Tab' },
  { value: ';', label: 'Semicolon' }
]

/** One field. `single`: the only column (Postgres then quotes `\.`, its end-of-data marker). */
export function csvField(value: string | null, delimiter: CsvDelimiter, single = false): string {
  if (value === null) return ''
  const quote =
    value === '' ||
    value.includes(delimiter) ||
    /["\r\n]/.test(value) ||
    (single && value === '\\.')
  return quote ? `"${value.replaceAll('"', '""')}"` : value
}

export function csvLine(values: (string | null)[], delimiter: CsvDelimiter): string {
  const single = values.length === 1
  return `${values.map((v) => csvField(v, delimiter, single)).join(delimiter)}\n`
}

/** The whole file: an optional header row of column names, then the rows. */
export function toCsv(
  columns: readonly string[],
  rows: readonly (string | null)[][],
  { delimiter, header }: CsvOptions
): string {
  let out = header ? csvLine([...columns], delimiter) : ''
  for (const r of rows) out += csvLine(r, delimiter)
  return out
}

/**
 * Default file name: the table a query reads from (`orders` for
 * `select … from public.orders`), else `fallback` (the console name), made
 * safe for a file system; `.tsv` for tab-separated output.
 */
export function csvFileName(sql: string, fallback: string, delimiter: CsvDelimiter): string {
  const from = /\bfrom\s+(?:(?:"(?:[^"]|"")+"|[\w$]+)\s*\.\s*)?("(?:[^"]|"")+"|[\w$]+)/i.exec(
    sql
  )?.[1]
  const raw = from
    ? from.startsWith('"')
      ? from.slice(1, -1).replaceAll('""', '"')
      : from.toLowerCase()
    : fallback
  const base =
    raw
      // biome-ignore lint/suspicious/noControlCharactersInRegex: strip control characters
      .replace(/[\u0000-\u001f/\\:*?"<>|]+/g, '_')
      .replace(/^[.\s]+|[.\s]+$/g, '') || 'result'
  return `${base}.${delimiter === '\t' ? 'tsv' : 'csv'}`
}

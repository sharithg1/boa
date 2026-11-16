// "Copy as" for a grid selection (DataGrip's data extractors): the selected
// rows × columns written as TSV, CSV, a Markdown table, JSON, SQL INSERT /
// UPDATE statements or an SQL IN list. Values are the grid's text (what
// Postgres prints), and the column type decides how each is written:
//
//   NULL     TSV / CSV: empty field (`""` is the empty string, as COPY writes)
//            Markdown: NULL · JSON: null · SQL: NULL
//   numbers  unquoted in JSON and SQL (when they are plain numeric literals;
//            NaN / Infinity / money text is quoted)
//   booleans `t` / `f` → true / false in JSON, TRUE / FALSE in SQL
//   json     embedded as JSON in JSON output (not a string), a string literal in SQL
//   arrays   a JSON array in JSON output when the literal parses, else text
//   others   strings (bytea stays its `\x…` text)
import { toCsv } from './csv'
import { sqlLiteral } from './gridEdits'
import { quoteIdent } from './sql'
import { type JsonNode, parseJson, valueKind, type ValueKind } from './values'

export type CopyFormat =
  | 'tsv'
  | 'tsv-header'
  | 'csv'
  | 'markdown'
  | 'json'
  | 'sql-insert'
  | 'sql-update'
  | 'sql-in'

export const COPY_FORMATS: { format: CopyFormat; label: string }[] = [
  { format: 'tsv', label: 'TSV' },
  { format: 'tsv-header', label: 'TSV with header' },
  { format: 'csv', label: 'CSV' },
  { format: 'markdown', label: 'Markdown table' },
  { format: 'json', label: 'JSON' },
  { format: 'sql-insert', label: 'SQL INSERT' },
  { format: 'sql-update', label: 'SQL UPDATE' },
  { format: 'sql-in', label: 'SQL IN list' }
]

/** Short name for toasts ("Copied 3 rows as JSON"). */
export const FORMAT_NAME: Record<CopyFormat, string> = {
  tsv: 'TSV',
  'tsv-header': 'TSV',
  csv: 'CSV',
  markdown: 'Markdown',
  json: 'JSON',
  'sql-insert': 'INSERT statements',
  'sql-update': 'UPDATE statements',
  'sql-in': 'SQL IN list'
}

/** INSERT target of console results (DataGrip's placeholder). */
export const PLACEHOLDER_TABLE = 'MY_TABLE'

export type CopyColumn = { name: string; type: string }

export type CopySelection = {
  /** The selected columns, in grid order. */
  columns: readonly CopyColumn[]
  /** The selected rows, values of the selected columns only (as displayed). */
  rows: readonly (readonly (string | null)[])[]
  /** Qualified, quoted table name (table tabs); console results use `MY_TABLE`. */
  table?: string
  /**
   * For UPDATE: the primary key, name → each selected row's value (from the
   * whole row, so key columns need not be selected).
   */
  key?: { columns: readonly CopyColumn[]; rows: readonly (readonly (string | null)[])[] }
  /** Cells still showing `<default>` (new rows): DEFAULT in SQL, else NULL. */
  isDefault?: (row: number, col: number) => boolean
}

/** Why a format can't be used for this selection, or null when it can. */
export function unavailable(format: CopyFormat, s: CopySelection): string | null {
  if (format === 'sql-in' && s.columns.length !== 1) return 'Select a single column'
  if (format === 'sql-update') {
    if (!s.table || !s.key?.columns.length) return 'Needs a table with a primary key'
    if (!updateColumns(s).length) return 'Select a column outside the primary key'
  }
  return null
}

/** The selection written in `format` (a trailing newline after each line / statement). */
export function copyAs(format: CopyFormat, s: CopySelection): string {
  const names = s.columns.map((c) => c.name)
  const rows = s.rows.map((r, i) => r.map((v, c) => (s.isDefault?.(i, c) ? null : v)))
  switch (format) {
    case 'tsv':
    case 'tsv-header':
      return toCsv(names, rows, { delimiter: '\t', header: format === 'tsv-header' })
    case 'csv':
      return toCsv(names, rows, { delimiter: ',', header: true })
    case 'markdown':
      return markdown(s.columns, rows)
    case 'json':
      return json(s.columns, rows)
    case 'sql-insert':
      return inserts(s)
    case 'sql-update':
      return updates(s)
    case 'sql-in':
      return `${s.rows.map((r) => sqlValue(r[0] ?? null, kindOf(s.columns[0]!))).join(', ')}\n`
  }
}

/** TSV of the values only: what ⌘C copies (a single cell: its raw text). */
export function copyPlain(s: CopySelection): string {
  if (s.rows.length === 1 && s.columns.length === 1) return s.rows[0]![0] ?? ''
  return copyAs('tsv', s)
}

const kindOf = (c: CopyColumn): ValueKind => valueKind(c.type)

// ── Markdown ──────────────────────────────────────────────────────────────

function mdCell(v: string | null): string {
  if (v === null) return 'NULL'
  return v
    .replace(/\\/g, '\\\\')
    .replace(/\|/g, '\\|')
    .replace(/\r\n|\r|\n/g, '<br>')
}

function markdown(columns: readonly CopyColumn[], rows: (string | null)[][]): string {
  const line = (cells: string[]) => `| ${cells.join(' | ')} |\n`
  let out = line(columns.map((c) => mdCell(c.name)))
  out += line(columns.map((c) => (kindOf(c) === 'number' ? '---:' : '---')))
  for (const r of rows) out += line(r.map(mdCell))
  return out
}

// ── JSON ──────────────────────────────────────────────────────────────────

const NUMERIC = /^-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?$/
const SQL_NUMERIC = /^[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?$/

/** JSON text for a node, indented `depth` levels (literals kept as written). */
function writeNode(n: JsonNode, depth: number): string {
  const pad = '  '.repeat(depth + 1)
  const end = '  '.repeat(depth)
  if (n.t === 'object')
    return n.entries.length
      ? `{\n${n.entries.map(([k, v]) => `${pad}${JSON.stringify(k)}: ${writeNode(v, depth + 1)}`).join(',\n')}\n${end}}`
      : '{}'
  if (n.t === 'array')
    return n.items.length
      ? `[\n${n.items.map((v) => pad + writeNode(v, depth + 1)).join(',\n')}\n${end}]`
      : '[]'
  return n.src
}

/** One value as JSON, per its column kind. */
export function jsonValue(v: string | null, kind: ValueKind, type = '', depth = 0): string {
  if (v === null) return 'null'
  switch (kind) {
    case 'number':
      return NUMERIC.test(v) ? v : JSON.stringify(v)
    case 'boolean': {
      const b = boolOf(v)
      return b === null ? JSON.stringify(v) : String(b)
    }
    case 'json':
      try {
        return writeNode(parseJson(v), depth)
      } catch {
        return JSON.stringify(v)
      }
    case 'array': {
      const items = parsePgArray(v)
      if (items === null) return JSON.stringify(v)
      const element = valueKind(type.replace(/^_/, '').replace(/\[\]$/, ''))
      return writeArray(items, element, depth)
    }
    default:
      return JSON.stringify(v)
  }
}

function writeArray(items: PgArray, kind: ValueKind, depth: number): string {
  if (!items.length) return '[]'
  const pad = '  '.repeat(depth + 1)
  const parts = items.map(
    (x) =>
      pad + (Array.isArray(x) ? writeArray(x, kind, depth + 1) : jsonValue(x, kind, '', depth + 1))
  )
  return `[\n${parts.join(',\n')}\n${'  '.repeat(depth)}]`
}

function json(columns: readonly CopyColumn[], rows: (string | null)[][]): string {
  if (!rows.length) return '[]\n'
  const objs = rows.map((r) => {
    const fields = columns.map(
      (c, i) => `    ${JSON.stringify(c.name)}: ${jsonValue(r[i] ?? null, kindOf(c), c.type, 2)}`
    )
    return fields.length ? `  {\n${fields.join(',\n')}\n  }` : '  {}'
  })
  return `[\n${objs.join(',\n')}\n]\n`
}

function boolOf(v: string): boolean | null {
  const t = v.toLowerCase()
  if (t === 't' || t === 'true') return true
  if (t === 'f' || t === 'false') return false
  return null
}

// ── Postgres array literals ───────────────────────────────────────────────

type PgArray = (string | null | PgArray)[]

/**
 * Parses a Postgres array literal (`{1,2}`, `{"a b",NULL}`, `{{1,2},{3,4}}`);
 * null when it isn't one (e.g. a `[0:1]={…}` bounds prefix).
 */
export function parsePgArray(text: string): PgArray | null {
  let i = 0
  const parse = (): PgArray | null => {
    if (text[i] !== '{') return null
    i++
    const out: PgArray = []
    if (text[i] === '}') {
      i++
      return out
    }
    for (;;) {
      if (text[i] === '{') {
        const sub = parse()
        if (sub === null) return null
        out.push(sub)
      } else if (text[i] === '"') {
        i++
        let s = ''
        while (i < text.length && text[i] !== '"') {
          if (text[i] === '\\') i++
          s += text[i] ?? ''
          i++
        }
        if (text[i] !== '"') return null
        i++
        out.push(s)
      } else {
        let s = ''
        while (i < text.length && text[i] !== ',' && text[i] !== '}') s += text[i++]
        s = s.trim()
        if (!s) return null
        out.push(s.toUpperCase() === 'NULL' ? null : s)
      }
      if (text[i] === ',') i++
      else if (text[i] === '}') {
        i++
        return out
      } else return null
    }
  }
  const out = parse()
  return out !== null && i === text.length ? out : null
}

// ── SQL ───────────────────────────────────────────────────────────────────

/** One value as an SQL literal, per its column kind. */
export function sqlValue(v: string | null, kind: ValueKind): string {
  if (v === null) return 'NULL'
  if (kind === 'number' && SQL_NUMERIC.test(v)) return v
  if (kind === 'boolean') {
    const b = boolOf(v)
    if (b !== null) return b ? 'TRUE' : 'FALSE'
  }
  return sqlLiteral(v)
}

function sqlCell(s: CopySelection, r: number, c: number): string {
  if (s.isDefault?.(r, c)) return 'DEFAULT'
  return sqlValue(s.rows[r]?.[c] ?? null, kindOf(s.columns[c]!))
}

function inserts(s: CopySelection): string {
  const table = s.table ?? PLACEHOLDER_TABLE
  const cols = s.columns.map((c) => quoteIdent(c.name)).join(', ')
  return s.rows
    .map(
      (r, i) =>
        `INSERT INTO ${table} (${cols}) VALUES (${r.map((_, c) => sqlCell(s, i, c)).join(', ')});\n`
    )
    .join('')
}

/** Selected columns an UPDATE sets: those outside the key. */
function updateColumns(s: CopySelection): number[] {
  const key = s.key?.columns.map((c) => c.name) ?? []
  return s.columns.map((c, i) => (key.includes(c.name) ? -1 : i)).filter((i) => i >= 0)
}

function updates(s: CopySelection): string {
  const why = unavailable('sql-update', s)
  if (why) throw new Error(why)
  const key = s.key!
  const sets = updateColumns(s)
  return s.rows
    .map((_, i) => {
      const set = sets.map((c) => `${quoteIdent(s.columns[c]!.name)} = ${sqlCell(s, i, c)}`)
      const where = key.columns.map((k, j) => {
        const v = key.rows[i]?.[j] ?? null
        const name = quoteIdent(k.name)
        return v === null ? `${name} IS NULL` : `${name} = ${sqlValue(v, kindOf(k))}`
      })
      return `UPDATE ${s.table} SET ${set.join(', ')} WHERE ${where.join(' AND ')};\n`
    })
    .join('')
}

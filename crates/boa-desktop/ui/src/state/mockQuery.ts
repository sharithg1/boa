// WHERE / ORDER BY for the browser mock (state/mockData.ts), enough to demo
// a table tab's filter bar: conditions `column <op> literal` (=, <>, !=, <,
// <=, >, >=, [not] like / ilike, is [not] null) joined by AND / OR, and
// ordering by column names. Anything else fails like a server would, with
// a note that the mock can't evaluate it.
import { clauseError, parseOrderBy, splitTopLevel, topLevelMatches } from '@/lib/tableQuery'
import { valueKind } from '@/lib/values'
import type { ResultColumn } from './types'

type Row = (string | null)[]
export type MockFailure = { kind: 'error'; message: string; code: string }

/** The WHERE and ORDER BY of what follows `from <table>` in a SELECT. */
export function tailClauses(tail: string): { where?: string; orderBy?: string } {
  const marks = topLevelMatches(tail, /\b(where|order\s+by|limit|offset)\b/gi)
  const clause = (kw: string) => {
    const i = marks.findIndex((m) => m.text.toLowerCase().replace(/\s+/g, ' ') === kw)
    if (i < 0) return undefined
    const m = marks[i]!
    return tail.slice(m.index + m.text.length, marks[i + 1]?.index ?? tail.length).trim()
  }
  return { where: clause('where'), orderBy: clause('order by') }
}

const unsupported = (what: string): MockFailure => ({
  kind: 'error',
  message: `The browser mock can't evaluate ${what} (it knows column = / <> / < / > / like / is null, joined by and / or, and ORDER BY column names)`,
  code: '0A000'
})

const missing = (name: string): MockFailure => ({
  kind: 'error',
  message: `column "${name}" does not exist`,
  code: '42703'
})

function columnIndex(columns: ResultColumn[], ident: string): number {
  const quoted = /^"(.*)"$/.exec(ident)
  const name = quoted ? quoted[1]!.replaceAll('""', '"') : ident.toLowerCase()
  return columns.findIndex((c) => c.name === name)
}

function compare(a: string, b: string, numeric: boolean): number {
  if (numeric && a.trim() !== '' && b.trim() !== '') {
    const x = Number(a)
    const y = Number(b)
    if (!Number.isNaN(x) && !Number.isNaN(y)) return x - y
  }
  return a < b ? -1 : a > b ? 1 : 0
}

const likeRe = (pattern: string, flags: string) =>
  new RegExp(
    `^${pattern
      .replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
      .replaceAll('%', '.*')
      .replaceAll('_', '.')}$`,
    `s${flags}`
  )

const PREDICATE =
  /^("(?:[^"]|"")+"|[A-Za-z_][\w$]*)\s*(=|<>|!=|<=|>=|<|>|not\s+i?like\b|i?like\b|is\s+not\s+null\b|is\s+null\b)\s*(.*)$/is

/** A row filter for one condition, or why the mock can't evaluate it. */
function predicate(text: string, columns: ResultColumn[]): ((row: Row) => boolean) | MockFailure {
  let t = text.trim()
  // `(a or b)`, but not `(a) or (b)`.
  while (/^\(.*\)$/s.test(t) && clauseError(t.slice(1, -1)) == null) t = t.slice(1, -1).trim()
  const ors = splitTopLevel(t, '\\bor\\b')
  if (ors.length > 1) return combine(ors, columns, 'some')
  const ands = splitTopLevel(t, '\\band\\b')
  if (ands.length > 1) return combine(ands, columns, 'every')
  const m = PREDICATE.exec(t)
  if (!m) return unsupported(`"${t}"`)
  const col = columnIndex(columns, m[1]!)
  if (col < 0) return missing(m[1]!.replace(/^"|"$/g, ''))
  const op = m[2]!.toLowerCase().replace(/\s+/g, ' ')
  if (op === 'is null') return m[3]!.trim() ? unsupported(`"${t}"`) : (row) => row[col] == null
  if (op === 'is not null') return m[3]!.trim() ? unsupported(`"${t}"`) : (row) => row[col] != null
  const raw = m[3]!.trim()
  const lit = /^'((?:[^']|'')*)'$/.exec(raw)
  const value = lit ? lit[1]!.replaceAll("''", "'") : /^-?\d+(\.\d+)?$/.test(raw) ? raw : null
  if (value == null) return unsupported(`"${t}"`)
  const numeric = valueKind(columns[col]!.type) === 'number'
  if (op.endsWith('like')) {
    const re = likeRe(value, op.includes('ilike') ? 'i' : '')
    const not = op.startsWith('not')
    return (row) => row[col] != null && re.test(row[col]!) !== not
  }
  const test: Record<string, (d: number) => boolean> = {
    '=': (d) => d === 0,
    '<>': (d) => d !== 0,
    '!=': (d) => d !== 0,
    '<': (d) => d < 0,
    '<=': (d) => d <= 0,
    '>': (d) => d > 0,
    '>=': (d) => d >= 0
  }
  const ok = test[op]!
  return (row) => row[col] != null && ok(compare(row[col]!, value, numeric))
}

function combine(
  parts: string[],
  columns: ResultColumn[],
  how: 'some' | 'every'
): ((row: Row) => boolean) | MockFailure {
  const fs: ((row: Row) => boolean)[] = []
  for (const p of parts) {
    const f = predicate(p, columns)
    if (typeof f !== 'function') return f
    fs.push(f)
  }
  return how === 'some' ? (row) => fs.some((f) => f(row)) : (row) => fs.every((f) => f(row))
}

/** `rows` filtered by `where` and sorted by `orderBy` (Postgres's NULLS LAST ascending). */
export function filterAndSort(
  columns: ResultColumn[],
  rows: Row[],
  { where, orderBy }: { where?: string; orderBy?: string }
): { kind: 'rows'; rows: Row[] } | MockFailure {
  let out = rows
  if (where) {
    const f = predicate(where, columns)
    if (typeof f !== 'function') return f
    out = out.filter(f)
  }
  const keys: { col: number; dir: 1 | -1; nullsFirst: boolean; numeric: boolean }[] = []
  for (const it of parseOrderBy(orderBy)) {
    if (it.column == null) return unsupported(`ORDER BY ${it.expr}`)
    const col = columns.findIndex((c) => c.name === it.column)
    if (col < 0) return missing(it.column)
    const dir = it.dir === 'desc' ? -1 : 1
    keys.push({
      col,
      dir,
      nullsFirst: it.nulls ? it.nulls === 'first' : dir === -1,
      numeric: valueKind(columns[col]!.type) === 'number'
    })
  }
  if (keys.length)
    out = [...out].sort((a, b) => {
      for (const k of keys) {
        const x = a[k.col] ?? null
        const y = b[k.col] ?? null
        if (x == null || y == null) {
          if (x === y) continue
          return (x == null) === k.nullsFirst ? -1 : 1
        }
        const d = compare(x, y, k.numeric)
        if (d) return d * k.dir
      }
      return 0
    })
  return { kind: 'rows', rows: out }
}

// Sort and filter of a table data tab (DataGrip's WHERE / ORDER BY bar):
// the SELECT a tab pages through, built from its free-form clauses, and the
// ORDER BY text that a column-header click (asc → desc → off) rewrites.
// Clauses are checked only for what would let them escape their place in
// the statement (a top-level `;`, unbalanced parentheses, an unterminated
// string or comment); everything else is left to the server, which reports
// a bad condition like any other query error.

import { qualified, quoteIdent } from './sql'

/** A table tab's clauses, as typed (without the WHERE / ORDER BY keyword). */
export type TableQuery = { where?: string; orderBy?: string }

export type SortDir = 'asc' | 'desc'

type Scan = {
  /** Per character: its parenthesis depth when it is code, -1 inside strings, quoted names and comments. */
  depth: number[]
  error: string | null
}

const isIdentChar = (c: string | undefined) => !!c && /[\w$À-￿]/.test(c)

/**
 * Walks `text` the way Postgres's lexer splits it: 'strings' (E'…' with
 * backslash escapes), "quoted names", $tag$ dollar quotes $tag$, -- line
 * and nested block comments, and parentheses.
 */
function scan(text: string): Scan {
  const n = text.length
  const depth = new Array<number>(n).fill(-1)
  let d = 0
  let i = 0
  while (i < n) {
    const c = text[i]!
    const next = text[i + 1]
    if (c === '-' && next === '-') {
      const end = text.indexOf('\n', i)
      i = end < 0 ? n : end
      continue
    }
    if (c === '/' && next === '*') {
      let level = 0
      let j = i
      while (j < n) {
        if (text[j] === '/' && text[j + 1] === '*') {
          level++
          j += 2
        } else if (text[j] === '*' && text[j + 1] === '/') {
          level--
          j += 2
          if (level === 0) break
        } else j++
      }
      if (level > 0) return { depth, error: 'Unterminated /* comment' }
      i = j
      continue
    }
    const eString = (c === 'E' || c === 'e') && next === "'" && !isIdentChar(text[i - 1])
    if (c === "'" || eString) {
      let j = eString ? i + 2 : i + 1
      let closed = false
      while (j < n) {
        const ch = text[j]!
        if (eString && ch === '\\') j += 2
        else if (ch === "'") {
          if (text[j + 1] === "'") j += 2
          else {
            j++
            closed = true
            break
          }
        } else j++
      }
      if (!closed) return { depth, error: 'Unterminated string literal' }
      i = j
      continue
    }
    if (c === '"') {
      let j = i + 1
      let closed = false
      while (j < n) {
        if (text[j] === '"') {
          if (text[j + 1] === '"') j += 2
          else {
            j++
            closed = true
            break
          }
        } else j++
      }
      if (!closed) return { depth, error: 'Unterminated quoted identifier' }
      i = j
      continue
    }
    if (c === '$' && !isIdentChar(text[i - 1])) {
      const tag = /^\$(?:[A-Za-z_]\w*)?\$/.exec(text.slice(i))?.[0]
      if (tag) {
        const end = text.indexOf(tag, i + tag.length)
        if (end < 0) return { depth, error: `Unterminated ${tag} string` }
        i = end + tag.length
        continue
      }
    }
    if (c === '(') {
      depth[i] = d++
    } else if (c === ')') {
      if (d === 0) return { depth, error: 'Unbalanced parentheses' }
      depth[i] = --d
    } else depth[i] = d
    i++
  }
  return { depth, error: d === 0 ? null : 'Unbalanced parentheses' }
}

/**
 * Why `clause` can't go into the generated SELECT as one clause of one
 * statement, or null when it can (whether it is valid SQL is the server's call).
 */
export function clauseError(clause: string): string | null {
  const { depth, error } = scan(clause)
  if (error) return error
  for (let i = 0; i < clause.length; i++)
    if (clause[i] === ';' && depth[i]! >= 0) return 'Only one condition: remove the ";"'
  return null
}

/** Offsets in `text` where `re` (global, sticky-free) matches top-level code. */
export function topLevelMatches(text: string, re: RegExp): { index: number; text: string }[] {
  const { depth, error } = scan(text)
  if (error) return []
  const out: { index: number; text: string }[] = []
  for (const m of text.matchAll(re))
    if (depth[m.index] === 0) out.push({ index: m.index, text: m[0] })
  return out
}

/** `text` split at top-level `sep` (a regex source, e.g. `,` or `\band\b`), parts trimmed. */
export function splitTopLevel(text: string, sep: string): string[] {
  const cuts = topLevelMatches(text, new RegExp(sep, 'gi'))
  if (!cuts.length) return [text.trim()].filter(Boolean)
  const parts: string[] = []
  let from = 0
  for (const c of cuts) {
    parts.push(text.slice(from, c.index))
    from = c.index + c.text.length
  }
  parts.push(text.slice(from))
  return parts.map((p) => p.trim())
}

/** The clause as typed, minus a leading `where` / `order by` someone typed out of habit. */
export function normalizeClause(text: string | undefined, keyword: 'where' | 'order by'): string {
  const t = (text ?? '').trim()
  const re = keyword === 'where' ? /^where\b\s*/i : /^order\s+by\b\s*/i
  return t.replace(re, '').trim()
}

/**
 * The statement a table tab pages through: `select *` with the tab's WHERE
 * and ORDER BY, one clause per line (so a trailing `--` comment can't swallow
 * the next). `error` is set when a clause must not be run.
 */
export function tableSelect(
  schema: string,
  table: string,
  q: TableQuery = {}
): { sql: string; error: string | null } {
  const where = normalizeClause(q.where, 'where')
  const orderBy = normalizeClause(q.orderBy, 'order by')
  const lines = [`select * from ${qualified(schema, table)}`]
  if (where) lines.push(`where ${where}`)
  if (orderBy) lines.push(`order by ${orderBy}`)
  const whereErr = where ? clauseError(where) : null
  const orderErr = orderBy ? clauseError(orderBy) : null
  return {
    sql: lines.join('\n'),
    error: whereErr ? `WHERE: ${whereErr}` : orderErr ? `ORDER BY: ${orderErr}` : null
  }
}

// ── ORDER BY items ────────────────────────────────────────────────────────

export type OrderItem = {
  /** The item as typed (or as rewritten by a header click). */
  text: string
  /** The sort expression, without direction / nulls. */
  expr: string
  dir: SortDir
  nulls?: 'first' | 'last'
  /** Column the expression names (`name`, `"Name"`), else null. */
  column: string | null
}

/** The column a bare or "quoted" identifier names (bare ones fold to lower case), else null. */
export function identName(expr: string): string | null {
  const e = expr.trim()
  if (/^[A-Za-z_À-￿][\w$À-￿]*$/.test(e)) return e.toLowerCase()
  const q = /^"((?:[^"]|"")+)"$/.exec(e)
  return q ? q[1]!.replaceAll('""', '"') : null
}

/** An ORDER BY clause's comma-separated items. */
export function parseOrderBy(orderBy: string | undefined): OrderItem[] {
  const text = normalizeClause(orderBy, 'order by')
  if (!text) return []
  // Can't be split reliably: one opaque item (a header click replaces it).
  const parts = scan(text).error ? [text] : splitTopLevel(text, ',').filter(Boolean)
  return parts.map((part) => {
    const m = /^([\s\S]*?)(?:\s+(asc|desc))?(?:\s+nulls\s+(first|last))?$/i.exec(part)!
    const expr = m[1]!.trim() || part
    return {
      text: part,
      expr,
      dir: (m[2]?.toLowerCase() as SortDir | undefined) ?? 'asc',
      nulls: m[3]?.toLowerCase() as OrderItem['nulls'],
      column: identName(expr)
    }
  })
}

/** Header arrows: each column the ORDER BY sorts by, its direction and 1-based position. */
export function sortState(
  orderBy: string | undefined
): Map<string, { dir: SortDir; rank: number }> {
  const out = new Map<string, { dir: SortDir; rank: number }>()
  parseOrderBy(orderBy).forEach((it, i) => {
    if (it.column != null && !out.has(it.column)) out.set(it.column, { dir: it.dir, rank: i + 1 })
  })
  return out
}

const render = (column: string, dir: SortDir, nulls?: 'first' | 'last') =>
  `${quoteIdent(column)}${dir === 'desc' ? ' desc' : ''}${nulls ? ` nulls ${nulls}` : ''}`

/**
 * A header click on `column` (DataGrip): ascending → descending → off. A
 * column already in the ORDER BY changes in place; another one replaces the
 * sort, or with `add` (⇧-click) is appended to it.
 */
export function cycleSort(orderBy: string | undefined, column: string, add = false): string {
  const items = parseOrderBy(orderBy)
  const i = items.findIndex((it) => it.column === column)
  let texts = items.map((it) => it.text)
  if (i >= 0) {
    const it = items[i]!
    if (it.dir === 'asc') texts[i] = render(column, 'desc', it.nulls)
    else texts = texts.filter((_, k) => k !== i)
  } else if (add) texts.push(render(column, 'asc'))
  else texts = [render(column, 'asc')]
  return texts.join(', ')
}

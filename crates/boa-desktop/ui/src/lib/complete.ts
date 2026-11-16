// Context-aware completion for the SQL console. No parser: a small lexer
// with offsets, then a look backwards from the caret within the current
// statement decides what fits there:
//
//   FROM / JOIN / UPDATE / INTO …   tables, views, CTEs, schemas
//   after a table reference         JOIN / WHERE / AS … keywords
//   SELECT / WHERE / ON / SET / BY  columns of the tables in the statement,
//                                   their aliases, functions, keywords
//   JOIN t ON                       first, conditions from foreign keys
//                                   between t and the tables before it
//   after an expression             AND / FROM / ORDER BY … keywords
//   alias. / table. / schema.       that table's columns, that schema's tables
//   INSERT INTO t (                 t's columns
//   :: / CAST(x AS                  type names
//
// Tables in scope are read from the whole statement, so `SELECT | FROM t`
// already knows t's columns. Kept free of the editor so it can be tested and
// reused by another editor.
import type { Schema, Table } from '@/state/types'
import { KEYWORDS, TYPES, qualified, quoteIdent } from './sql'

export type CompletionKind =
  | 'keyword'
  | 'schema'
  | 'table'
  | 'view'
  | 'cte'
  | 'column'
  | 'alias'
  | 'function'
  | 'type'
  /** A JOIN … ON condition from a foreign key. */
  | 'join'

export type Completion = {
  label: string
  kind: CompletionKind
  /** Text that replaces the word at the caret. */
  insert: string
  /** Caret offset into `insert` after inserting (default: its end). */
  caret?: number
  /** Short trailing hint: a column's type and table, a table's schema. */
  detail?: string
  /** Primary-key column. */
  pk?: boolean
}

export type CompletionResult = {
  /** Start of the word being completed (an opening `"` included). */
  from: number
  /** End of the word at the caret, for replacing the whole word. */
  to: number
  prefix: string
  items: Completion[]
}

export type CompleteOptions = {
  /** Ranked first among schemas (the console's default schema). */
  defaultSchema?: string
  /** Schemas whose tables need no qualifier (Postgres search_path). */
  searchPath?: string[]
  /** Invoked by the user (Ctrl+Space): show items for an empty prefix too. */
  explicit?: boolean
  limit?: number
}

// --- lexer ------------------------------------------------------------------

type TokType = 'word' | 'ident' | 'string' | 'comment' | 'number' | 'punct' | 'other'
type Tok = {
  type: TokType
  start: number
  end: number
  /** Lower-cased word, unquoted identifier, or the punctuation character. */
  name: string
  /** Unterminated, or a line comment: a caret at `end` is still inside. */
  open?: boolean
}

const isIdentStart = (c: string | undefined) => !!c && /[A-Za-z_À-￿]/.test(c)
const isIdent = (c: string | undefined) => !!c && /[\w$À-￿]/.test(c)

function lex(sql: string): Tok[] {
  const out: Tok[] = []
  const n = sql.length
  const add = (type: TokType, start: number, end: number, name = '', open = false) =>
    out.push({ type, start, end, name, open })
  let i = 0
  while (i < n) {
    const c = sql[i]!
    const next = sql[i + 1]
    if (/\s/.test(c)) {
      i++
      continue
    }
    if (c === '-' && next === '-') {
      const e = sql.indexOf('\n', i)
      const j = e < 0 ? n : e
      add('comment', i, j, '', true)
      i = j
      continue
    }
    if (c === '/' && next === '*') {
      const e = sql.indexOf('*/', i + 2)
      const j = e < 0 ? n : e + 2
      add('comment', i, j, '', e < 0)
      i = j
      continue
    }
    if (c === "'" || (/[eEnN]/.test(c) && next === "'")) {
      let j = c === "'" ? i + 1 : i + 2
      let closed = false
      while (j < n) {
        if (sql[j] === "'") {
          if (sql[j + 1] === "'") j += 2
          else {
            j++
            closed = true
            break
          }
        } else j++
      }
      add('string', i, j, '', !closed)
      i = j
      continue
    }
    if (c === '$') {
      const tag = /^\$[A-Za-z_]*\$/.exec(sql.slice(i, i + 64))
      if (tag) {
        const e = sql.indexOf(tag[0], i + tag[0].length)
        const j = e < 0 ? n : e + tag[0].length
        add('string', i, j, '', e < 0)
        i = j
        continue
      }
    }
    if (c === '"' || c === '`') {
      let j = i + 1
      let closed = false
      while (j < n) {
        if (sql[j] === c) {
          if (sql[j + 1] === c) j += 2
          else {
            j++
            closed = true
            break
          }
        } else j++
      }
      const body = sql.slice(i + 1, closed ? j - 1 : j).replaceAll(c + c, c)
      add('ident', i, j, body.toLowerCase(), !closed)
      i = j
      continue
    }
    if (/\d/.test(c)) {
      let j = i + 1
      while (j < n && /[\w.]/.test(sql[j]!)) j++
      add('number', i, j)
      i = j
      continue
    }
    if (isIdentStart(c)) {
      let j = i + 1
      while (j < n && isIdent(sql[j])) j++
      add('word', i, j, sql.slice(i, j).toLowerCase())
      i = j
      continue
    }
    if (c === '$' || c === ':' || c === '@') {
      // $1, :name, @name parameters; `::` stays punctuation.
      const m = /^(\$\d+|[:@][A-Za-z_]\w*)/.exec(sql.slice(i, i + 64))
      if (m && !(c === ':' && (next === ':' || sql[i - 1] === ':'))) {
        add('other', i, i + m[0].length)
        i += m[0].length
        continue
      }
    }
    add('punct', i, i + 1, c)
    i++
  }
  return out
}

const isName = (t: Tok | undefined) =>
  !!t && (t.type === 'ident' || (t.type === 'word' && !KEYWORDS.has(t.name)))
const isPunct = (t: Tok | undefined, ch: string) => !!t && t.type === 'punct' && t.name === ch
const isWord = (t: Tok | undefined, ...ws: string[]) =>
  !!t && t.type === 'word' && ws.includes(t.name)

// --- vocabulary -------------------------------------------------------------

const words = (s: string) =>
  s
    .split(',')
    .map((w) => w.trim())
    .filter(Boolean)

const START = words(`select, insert into, update, delete from, with, explain, explain analyze,
  create table, create index, create view, alter table, drop table, truncate, begin, commit,
  rollback, values, show, set, vacuum, analyze, table`)

const AFTER_TABLE = words(`as, join, inner join, left join, right join, full join, cross join,
  natural join, on, using, where, group by, order by, having, limit, offset, union, union all,
  set, values, returning, for update`)

const AFTER_EXPR = words(`and, or, not, as, from, where, join, inner join, left join, on,
  group by, order by, having, limit, offset, asc, desc, nulls first, nulls last, is null,
  is not null, in, not in, like, ilike, between, union, union all, except, intersect,
  returning, then, else, end, when, over, filter`)

const EXPR = words(`distinct, case, when, then, else, end, not, exists, null, true, false,
  select, interval, and, or`)

const FUNCTIONS = words(`count, sum, avg, min, max, coalesce, nullif, greatest, least, now,
  lower, upper, length, trim, substring, replace, concat, date_trunc, extract, to_char, to_date,
  to_timestamp, round, floor, ceil, abs, string_agg, array_agg, json_agg, jsonb_agg,
  json_build_object, jsonb_build_object, row_number, rank, dense_rank, lag, lead, first_value,
  last_value, generate_series, gen_random_uuid, age, date_part`)

/** Keyword that opens the clause the caret is in. */
const TABLE_CLAUSE = new Set(['from', 'join', 'update', 'into', 'table', 'truncate'])
const COLUMN_CLAUSE = new Set(
  words(`select, where, on, and, or, by, having, set, returning, when, then, else, case, not,
    between, like, ilike, distinct, in, is, using, over, partition`)
)
const NO_COMPLETION = new Set(['as', 'limit', 'offset', 'with', 'values', 'fetch'])
/** Keywords that end an expression, after which keywords (not columns) follow. */
const EXPR_END = new Set(['null', 'true', 'false', 'asc', 'desc', 'end', 'first', 'last'])

// --- scope ------------------------------------------------------------------

type Ref = {
  /** Table name as written (lower-cased). */
  name: string
  alias?: string
  schema?: string
  table?: Table
  /** INSERT INTO target. */
  insert: boolean
  /** Index (in the statement's tokens) of the last token of the reference. */
  endTok: number
}

type Catalog = { schemas: Schema[]; searchPath: string[]; defaultSchema?: string }

function findTable(cat: Catalog, parts: string[]): { schema: string; table: Table } | undefined {
  const name = parts[parts.length - 1]!
  const look = (s: Schema) => {
    const table = s.tables.find((t) => t.name.toLowerCase() === name)
    return table ? { schema: s.name, table } : undefined
  }
  if (parts.length >= 2) {
    const s = cat.schemas.find((x) => x.name.toLowerCase() === parts[parts.length - 2])
    return s && look(s)
  }
  const order = [...cat.searchPath, cat.defaultSchema ?? '']
  for (const name of order) {
    const s = cat.schemas.find((x) => x.name === name)
    const hit = s && look(s)
    if (hit) return hit
  }
  for (const s of cat.schemas) {
    const hit = look(s)
    if (hit) return hit
  }
  return undefined
}

/** Tables (with aliases) and CTE names the statement refers to, anywhere in it. */
function scope(stmt: Tok[], cat: Catalog): { refs: Ref[]; ctes: string[] } {
  const refs: Ref[] = []
  const ctes: string[] = []
  for (let k = 0; k < stmt.length; k++) {
    const t = stmt[k]!
    // name AS ( … ) after WITH / RECURSIVE / a comma
    if (
      isName(t) &&
      isWord(stmt[k + 1], 'as') &&
      isPunct(stmt[k + 2], '(') &&
      (isWord(stmt[k - 1], 'with', 'recursive') || isPunct(stmt[k - 1], ','))
    ) {
      ctes.push(t.name)
      continue
    }
    if (t.type !== 'word' || !TABLE_CLAUSE.has(t.name)) continue
    let j = k + 1
    for (;;) {
      if (isWord(stmt[j], 'only')) j++
      if (!isName(stmt[j])) break
      const parts = [stmt[j]!.name]
      while (isPunct(stmt[j + 1], '.') && isName(stmt[j + 2])) {
        parts.push(stmt[j + 2]!.name)
        j += 2
      }
      let endTok = j
      j++
      let alias: string | undefined
      if (isWord(stmt[j], 'as')) j++
      if (isName(stmt[j]) && !isPunct(stmt[j + 1], '.') && !isPunct(stmt[j + 1], '(')) {
        alias = stmt[j]!.name
        endTok = j
        j++
      }
      const hit = parts.length === 1 && ctes.includes(parts[0]!) ? undefined : findTable(cat, parts)
      refs.push({
        name: parts[parts.length - 1]!,
        alias,
        schema: hit?.schema,
        table: hit?.table,
        insert: t.name === 'into',
        endTok
      })
      if (t.name === 'from' && isPunct(stmt[j], ',')) j++
      else break
    }
  }
  return { refs, ctes }
}

// --- matching ---------------------------------------------------------------

/** How well `label` matches what was typed: 0 none … 5 exact. */
export function matchTier(label: string, typed: string): number {
  if (!typed) return 1
  // `u.created_at` also matches as `created_at`.
  const dot = label.lastIndexOf('.')
  if (dot > 0) return Math.max(tier(label, typed), tier(label.slice(dot + 1), typed))
  return tier(label, typed)
}

function tier(label: string, typed: string): number {
  const l = label.toLowerCase()
  const q = typed.toLowerCase()
  if (l === q) return 5
  if (l.startsWith(q)) return 4
  if (segmentsMatch(l.split(/[_\s.]+/).filter(Boolean), q)) return 3
  // A substring anywhere is noise for one or two letters.
  if (q.length >= 3 && l.includes(q)) return 2
  return 0
}

/** `uid` → user_id, `crat` → created_at: each piece is a prefix of a later segment. */
function segmentsMatch(segs: string[], q: string): boolean {
  if (q.length < 2 || segs.length < 2) return false
  const go = (si: number, qi: number): boolean => {
    if (qi === q.length) return true
    if (si === segs.length) return false
    const seg = segs[si]!
    for (let len = Math.min(seg.length, q.length - qi); len >= 1; len--) {
      if (seg.startsWith(q.slice(qi, qi + len)) && go(si + 1, qi + len)) return true
    }
    return si > 0 && go(si + 1, qi)
  }
  return go(0, 0)
}

// --- complete ---------------------------------------------------------------

type Mode =
  | { kind: 'start' }
  | { kind: 'table' }
  | { kind: 'afterTable' }
  | { kind: 'column' }
  | { kind: 'afterExpr' }
  | { kind: 'insertColumns'; ref: Ref }
  | { kind: 'type' }

type Ranked = Completion & { priority: number }

export function complete(
  sql: string,
  pos: number,
  schemas: Schema[],
  opts: CompleteOptions = {}
): CompletionResult | null {
  const cat: Catalog = {
    schemas,
    searchPath: opts.searchPath ?? ['public'],
    defaultSchema: opts.defaultSchema
  }
  const toks = lex(sql)

  // The word at the caret, possibly an open "quoted identifier.
  let from = pos
  let to = pos
  let prefix: string
  let quoted = false
  const inside = toks.find((t) => pos > t.start && (pos < t.end || (pos === t.end && t.open)))
  if (inside?.type === 'string' || inside?.type === 'comment' || inside?.type === 'other')
    return null
  if (inside?.type === 'number') return null
  if (inside?.type === 'ident') {
    quoted = true
    from = inside.start
    to = inside.end
    prefix = sql.slice(inside.start + 1, pos)
  } else {
    while (from > 0 && isIdent(sql[from - 1])) from--
    while (to < sql.length && isIdent(sql[to])) to++
    prefix = sql.slice(from, pos)
    if (/^\d/.test(prefix)) return null
  }

  // Current statement, comments dropped.
  let lo = 0
  let hi = toks.length
  toks.forEach((t, i) => {
    if (!isPunct(t, ';')) return
    if (t.end <= from) lo = i + 1
    else if (t.start >= pos && hi === toks.length) hi = i
  })
  const stmt = toks.slice(lo, hi).filter((t) => t.type !== 'comment')
  const pre = stmt.filter((t) => t.end <= from)
  const { refs, ctes } = scope(stmt, cat)
  const last = pre[pre.length - 1]

  const items: Ranked[] = []
  const add = (c: Completion, priority: number) => items.push({ ...c, priority })

  const addColumns = (ref: Ref, priority: number, qualify?: string) => {
    for (const c of ref.table?.columns ?? []) {
      add(
        {
          label: c.name,
          kind: 'column',
          insert: qualify ? `${quoteIdent(qualify)}.${quoteIdent(c.name)}` : quoteIdent(c.name),
          detail: `${c.type} · ${ref.alias ?? ref.name}`,
          pk: c.pk
        },
        priority
      )
    }
  }
  const tableItem = (schema: string, t: Table, priority: number, qualify: boolean) =>
    add(
      {
        label: t.name,
        kind: t.kind === 'view' ? 'view' : 'table',
        insert: qualify ? qualified(schema, t.name) : quoteIdent(t.name),
        detail: schema
      },
      priority
    )
  const addTables = (priority: number) => {
    for (const s of schemas) {
      const onPath = cat.searchPath.includes(s.name)
      const bonus = s.name === cat.defaultSchema ? 0.5 : onPath ? 0.25 : 0
      for (const t of s.tables) tableItem(s.name, t, priority + bonus, !onPath)
    }
  }

  // --- qualified: alias. / table. / schema. / schema.table.
  if (!quoted && isPunct(last, '.') && last!.end === from) {
    const parts: string[] = []
    let k = pre.length - 2
    while (k >= 0 && isNameOrWord(pre[k])) {
      parts.unshift(pre[k]!.name)
      if (isPunct(pre[k - 1], '.') && pre[k - 1]!.end === pre[k]!.start) k -= 2
      else break
    }
    if (!parts.length) return null
    if (parts.length === 1) {
      const p = parts[0]!
      const ref = refs.find((r) => r.alias === p) ?? refs.find((r) => !r.alias && r.name === p)
      const direct = ref?.table ? ref : tableRef(cat, [p])
      const schema = schemas.find((s) => s.name.toLowerCase() === p)
      if (direct?.table) addColumns({ ...direct, alias: undefined, name: p }, 5)
      else if (schema) for (const t of schema.tables) tableItem(schema.name, t, 4, false)
    } else {
      const ref = tableRef(cat, parts.slice(-2))
      if (ref) addColumns(ref, 5)
    }
    return finish(items, from, to, prefix, opts, true)
  }

  const mode = modeAt(pre, refs, opts.explicit)
  if (!mode) return null
  const upper = upperKeywords(sql, pre, prefix)

  if (mode.kind === 'start') {
    for (const k of START) add(keyword(k, upper), 1)
  } else if (mode.kind === 'table') {
    for (const c of ctes) add({ label: c, kind: 'cte', insert: quoteIdent(c), detail: 'cte' }, 5)
    addTables(4)
    for (const s of schemas)
      add({ label: s.name, kind: 'schema', insert: `${quoteIdent(s.name)}.` }, 2)
  } else if (mode.kind === 'afterTable') {
    for (const k of AFTER_TABLE) add(keyword(k, upper), 1)
  } else if (mode.kind === 'afterExpr') {
    for (const k of AFTER_EXPR) add(keyword(k, upper), 1)
  } else if (mode.kind === 'insertColumns') {
    addColumns(mode.ref, 5)
  } else if (mode.kind === 'type') {
    for (const t of TYPES) add({ label: t, kind: 'type', insert: t }, 3)
  } else {
    // JOIN t ON |: the foreign keys between t and the tables before it, first.
    if (isWord(last, 'on'))
      for (const cond of joinConditions(refs, pre.length - 1, upper))
        add({ label: cond, kind: 'join', insert: cond, detail: 'foreign key' }, 6)
    // Columns of every table in the statement. A name two tables share is
    // inserted qualified, so the query stays unambiguous.
    const resolved = refs.filter((r) => r.table)
    const count = new Map<string, number>()
    for (const r of resolved)
      for (const c of r.table!.columns) count.set(c.name, (count.get(c.name) ?? 0) + 1)
    for (const r of resolved) {
      for (const c of r.table!.columns) {
        const shared = (count.get(c.name) ?? 0) > 1
        add(
          {
            label: shared ? `${r.alias ?? r.name}.${c.name}` : c.name,
            kind: 'column',
            insert: shared
              ? `${quoteIdent(r.alias ?? r.name)}.${quoteIdent(c.name)}`
              : quoteIdent(c.name),
            detail: `${c.type} · ${r.alias ?? r.name}`,
            pk: c.pk
          },
          5
        )
      }
    }
    for (const r of refs) {
      if (r.alias) add({ label: r.alias, kind: 'alias', insert: r.alias, detail: r.name }, 4)
    }
    // Nothing in scope yet (SELECT typed before FROM): offer every column on
    // the search path once something is typed.
    if (!resolved.length && prefix) {
      for (const s of schemas) {
        if (!cat.searchPath.includes(s.name) && s.name !== cat.defaultSchema) continue
        for (const t of s.tables)
          addColumns({ name: t.name, table: t, insert: false, endTok: -1 }, 3.5)
      }
    }
    for (const f of FUNCTIONS)
      add({ label: f, kind: 'function', insert: `${f}()`, caret: f.length + 1 }, 3)
    for (const k of EXPR) add(keyword(k, upper), 2)
    if (prefix) addTables(1)
  }
  return finish(items, from, to, prefix, opts, false)
}

const isNameOrWord = (t: Tok | undefined) => !!t && (t.type === 'ident' || t.type === 'word')

/**
 * `a.x = b.y` conditions for `JOIN t ON`, from the foreign keys either way
 * between t (the reference ending just before ON, token `onTok`) and each
 * table referenced before it; the joined table's side comes first.
 */
function joinConditions(refs: Ref[], onTok: number, upper: boolean): string[] {
  const joined = refs.find((r) => r.endTok === onTok - 1)
  if (!joined?.table) return []
  const q = (r: Ref) => quoteIdent(r.alias ?? r.name)
  const and = upper ? ' AND ' : ' and '
  const fks = (from: Ref, to: Ref) =>
    (from.table?.constraints ?? []).filter((c) => {
      const r = c.references
      return (
        c.kind === 'f' &&
        !!r &&
        r.table === to.table?.name &&
        r.schema === to.schema &&
        r.columns.length === c.columns.length
      )
    })
  const cond = (a: Ref, aCols: string[], b: Ref, bCols: string[]) =>
    aCols.map((c, i) => `${q(a)}.${quoteIdent(c)} = ${q(b)}.${quoteIdent(bCols[i]!)}`).join(and)
  const out: string[] = []
  for (const other of refs) {
    if (other === joined || !other.table || other.insert || other.endTok >= joined.endTok) continue
    for (const fk of fks(joined, other))
      out.push(cond(joined, fk.columns, other, fk.references!.columns))
    for (const fk of fks(other, joined))
      out.push(cond(joined, fk.references!.columns, other, fk.columns))
  }
  return out
}

function tableRef(cat: Catalog, parts: string[]): Ref | undefined {
  const hit = findTable(cat, parts)
  return (
    hit && { name: hit.table.name, schema: hit.schema, table: hit.table, insert: false, endTok: -1 }
  )
}

/** What belongs at the caret, from the clause it is in. */
function modeAt(pre: Tok[], refs: Ref[], explicit?: boolean): Mode | null {
  const last = pre[pre.length - 1]
  if (isPunct(last, ':') && isPunct(pre[pre.length - 2], ':')) return { kind: 'type' }

  // Walk back to the keyword opening this clause, skipping (…) groups; an
  // unmatched `(` means the caret is inside parentheses.
  let depth = 0
  let clause: Tok | undefined
  let paren = -1
  for (let k = pre.length - 1; k >= 0; k--) {
    const t = pre[k]!
    if (isPunct(t, ')')) depth++
    else if (isPunct(t, '(')) {
      if (depth === 0) {
        paren = k
        break
      }
      depth--
    } else if (depth === 0 && t.type === 'word') {
      if (TABLE_CLAUSE.has(t.name) || COLUMN_CLAUSE.has(t.name) || NO_COMPLETION.has(t.name)) {
        clause = t
        break
      }
      if (t.name === 'explain') break
    }
  }

  if (!clause) {
    if (paren < 0) return { kind: 'start' }
    const ref = refs.find((r) => r.insert && r.endTok === paren - 1)
    if (ref?.table) return { kind: 'insertColumns', ref }
    if (isPunct(last, ',') && ref) return { kind: 'insertColumns', ref }
    return afterExpr(pre) ? { kind: 'afterExpr' } : { kind: 'column' }
  }
  if (clause.name === 'as') {
    // CAST(x AS |): a type. Otherwise an alias is being named.
    const open = enclosingParen(pre, pre.indexOf(clause))
    if (open > 0 && isWord(pre[open - 1], 'cast')) return { kind: 'type' }
    return explicit && last !== clause ? { kind: 'afterExpr' } : null
  }
  if (NO_COMPLETION.has(clause.name)) return null
  if (TABLE_CLAUSE.has(clause.name)) {
    return last === clause || isPunct(last, ',') || isWord(last, 'only')
      ? { kind: 'table' }
      : { kind: 'afterTable' }
  }
  if (last !== clause && afterExpr(pre)) return { kind: 'afterExpr' }
  return { kind: 'column' }
}

/** Index of the unmatched `(` before index `k`, or -1. */
function enclosingParen(pre: Tok[], k: number): number {
  let depth = 0
  for (let i = k - 1; i >= 0; i--) {
    if (isPunct(pre[i], ')')) depth++
    else if (isPunct(pre[i], '(')) {
      if (depth === 0) return i
      depth--
    }
  }
  return -1
}

/** The previous token ends an expression: `x |`, `'a' |`, `f(…) |`, `select * |`. */
function afterExpr(pre: Tok[]): boolean {
  const last = pre[pre.length - 1]
  if (!last) return false
  if (last.type === 'ident' || last.type === 'number' || last.type === 'string') return true
  if (last.type === 'other') return true
  if (last.type === 'word') return !KEYWORDS.has(last.name) || EXPR_END.has(last.name)
  if (isPunct(last, ')')) return true
  if (isPunct(last, '*')) {
    const before = pre[pre.length - 2]
    return isWord(before, 'select', 'distinct') || isPunct(before, ',') || isPunct(before, '.')
  }
  return false
}

/** Keywords take the case being typed, else that of the statement's last keyword. */
function upperKeywords(sql: string, pre: Tok[], prefix: string): boolean {
  if (/[a-z]/i.test(prefix)) return prefix === prefix.toUpperCase()
  const kw = [...pre].reverse().find((t) => t.type === 'word' && KEYWORDS.has(t.name))
  if (!kw) return false
  const text = sql.slice(kw.start, kw.end)
  return text === text.toUpperCase()
}

const keyword = (k: string, upper: boolean): Completion => ({
  label: k,
  kind: 'keyword',
  insert: upper ? k.toUpperCase() : k
})

function finish(
  items: Ranked[],
  from: number,
  to: number,
  prefix: string,
  opts: CompleteOptions,
  qualified: boolean
): CompletionResult | null {
  if (!prefix && !opts.explicit && !qualified) return null
  const seen = new Set<string>()
  const ranked = items
    .map((c, i) => ({ c, i, tier: matchTier(c.label, prefix) }))
    .filter(({ c, tier }) => {
      if (!tier) return false
      const id = `${c.kind}:${c.label}:${c.detail ?? ''}`
      if (seen.has(id)) return false
      seen.add(id)
      return true
    })
    // Nothing typed: keep the given order (table definition order, curated
    // keyword lists). Otherwise shorter, then alphabetical.
    .sort(
      (a, b) =>
        b.tier - a.tier ||
        b.c.priority - a.c.priority ||
        (prefix
          ? a.c.label.length - b.c.label.length || a.c.label.localeCompare(b.c.label)
          : a.i - b.i)
    )
    .slice(0, opts.limit ?? 200)
    .map(({ c: { priority: _, ...c } }) => c)
  if (!ranked.length) return null
  // Typing a word that is already complete should not pop a list up that
  // swallows the next Enter.
  if (
    !opts.explicit &&
    ranked[0]!.label.toLowerCase() === prefix.toLowerCase() &&
    ranked.length === 1
  )
    return null
  return { from, to, prefix, items: ranked }
}

// Write guard rails: which statements are destructive enough to confirm
// before they run, and which would talk a read-only session out of being
// read-only. Works on the editor's tokenizer (lib/sql.ts) and statement
// splitter (lib/statements.ts), never on raw text, so comments, strings,
// quoted names and dollar quotes cannot fool it.
//
// Destructive: UPDATE / DELETE without a WHERE (also inside a CTE or
// EXPLAIN ANALYZE, which run them), TRUNCATE, DROP of anything, and ALTER
// TABLE … DROP COLUMN. Not looked into: the bodies of DO blocks and
// functions (they are strings).

import { tokenize } from './sql'
import { splitStatements } from './statements'

export type RiskKind = 'no-where' | 'truncate' | 'drop' | 'drop-column'

export type Risk = {
  kind: RiskKind
  /** What is risky, for the confirmation dialog. */
  message: string
  /** The statement it was found in. */
  statement: string
}

type Tok = { kind: 'word' | 'quoted' | 'punct' | 'string' | 'other'; text: string; raw: string }

const WORD_START = /^[A-Za-z_À-￿]/
const PIECES = /"(?:[^"]|"")*"?|`[^`]*`?|[A-Za-z_À-￿][\w$À-￿]*|::|\S/g

/** Significant tokens: no comments or blanks; words lower-cased, one per token. */
function lex(sql: string): Tok[] {
  const out: Tok[] = []
  for (const t of tokenize(sql)) {
    if (t.kind === 'comment') continue
    if (t.kind === 'string') {
      out.push({ kind: 'string', text: t.text, raw: t.text })
      continue
    }
    if (t.kind === 'number' || t.kind === 'param') {
      out.push({ kind: 'other', text: t.text, raw: t.text })
      continue
    }
    // Plain runs merge words, blanks and quoted names: split them again.
    for (const m of t.text.matchAll(PIECES)) {
      const raw = m[0]
      if (raw[0] === '"' || raw[0] === '`') out.push({ kind: 'quoted', text: raw, raw })
      else if (WORD_START.test(raw)) out.push({ kind: 'word', text: raw.toLowerCase(), raw })
      else out.push({ kind: 'punct', text: raw, raw })
    }
  }
  return out
}

const isName = (t: Tok | undefined) => t?.kind === 'word' || t?.kind === 'quoted'

/** A possibly qualified name starting at `i`: its text and the index after it. */
function nameAt(toks: Tok[], i: number): { name: string; next: number } | null {
  if (!isName(toks[i])) return null
  let name = toks[i]!.raw
  let k = i + 1
  while (toks[k]?.text === '.' && isName(toks[k + 1])) {
    name += `.${toks[k + 1]!.raw}`
    k += 2
  }
  return { name, next: k }
}

/** Index after the `)` matching the `(` at `i`. */
function skipParens(toks: Tok[], i: number): number {
  let depth = 0
  for (let k = i; k < toks.length; k++) {
    if (toks[k]!.text === '(') depth++
    else if (toks[k]!.text === ')' && --depth === 0) return k + 1
  }
  return toks.length
}

/**
 * Where the statement really starts: after EXPLAIN ANALYZE options. Plain
 * EXPLAIN only plans, so it returns -1 (nothing runs).
 */
function bodyStart(toks: Tok[]): number {
  if (toks[0]?.text !== 'explain') return 0
  let analyze = false
  let i = 1
  if (toks[1]?.text === '(') {
    const end = skipParens(toks, 1)
    for (let k = 2; k < end; k++) {
      if (toks[k]!.text !== 'analyze' && toks[k]!.text !== 'analyse') continue
      const v = toks[k + 1]?.text
      analyze = !(v === 'false' || v === 'off' || v === '0')
    }
    i = end
  } else {
    while (['analyze', 'analyse', 'verbose'].includes(toks[i]?.text ?? '')) {
      if (toks[i]!.text !== 'verbose') analyze = true
      i++
    }
  }
  return analyze ? i : -1
}

/** UPDATE / DELETE at `i` (depth `depth`): is there a WHERE at its own level? */
function hasWhere(toks: Tok[], i: number): boolean {
  let depth = 0
  for (let k = i + 1; k < toks.length; k++) {
    const t = toks[k]!.text
    if (t === '(') depth++
    else if (t === ')') {
      if (--depth < 0) return false // end of the CTE / subquery it is in
    } else if (depth === 0 && t === 'where' && toks[k]!.kind === 'word') return true
  }
  return false
}

const DROP_OBJECT_WORDS = new Set(
  `access aggregate cast collation conversion data database default domain event extension
  foreign function group index language materialized method operator owned policy procedure
  publication role routine rule schema sequence server statistics subscription table tablespace
  text search configuration dictionary parser template transform trigger type user mapping view
  wrapper class family concurrently`.split(/\s+/)
)

function risksOf(statement: string): Risk[] {
  const toks = lex(statement)
  const start = bodyStart(toks)
  if (start < 0) return []
  const out: Risk[] = []
  const add = (kind: RiskKind, message: string) => out.push({ kind, message, statement })
  const first = toks[start]?.text

  if (first === 'truncate') {
    let k = start + 1
    if (toks[k]?.text === 'table') k++
    const names: string[] = []
    while (k < toks.length) {
      if (toks[k]!.text === 'only') k++
      const n = nameAt(toks, k)
      if (!n) break
      names.push(n.name)
      k = n.next
      if (toks[k]?.text !== ',') break
      k++
    }
    add('truncate', `TRUNCATE removes every row of ${names.join(', ') || 'the table'}`)
  }

  if (first === 'drop') {
    let k = start + 1
    const what: string[] = []
    while (toks[k]?.kind === 'word' && DROP_OBJECT_WORDS.has(toks[k]!.text)) {
      what.push(toks[k]!.raw.toUpperCase())
      k++
    }
    if (toks[k]?.text === 'if' && toks[k + 1]?.text === 'exists') k += 2
    const names: string[] = []
    for (
      let n = nameAt(toks, k);
      n;
      n = toks[n.next]?.text === ',' ? nameAt(toks, n.next + 1) : null
    )
      names.push(n.name)
    const cascade = toks.some((t) => t.kind === 'word' && t.text === 'cascade')
    const target = ['DROP', ...what, names.join(', ')].filter(Boolean).join(' ')
    add(
      'drop',
      `${target} cannot be undone${cascade ? '; CASCADE also drops everything that depends on it' : ''}`
    )
  }

  if (first === 'alter' && toks[start + 1]?.text === 'table') {
    let k = start + 2
    if (toks[k]?.text === 'if' && toks[k + 1]?.text === 'exists') k += 2
    if (toks[k]?.text === 'only') k++
    const table = nameAt(toks, k)?.name ?? 'the table'
    let depth = 0
    for (let j = k; j < toks.length; j++) {
      const t = toks[j]!
      if (t.text === '(') depth++
      else if (t.text === ')') depth--
      if (depth !== 0 || t.kind !== 'word' || t.text !== 'drop') continue
      let c = j + 1
      const next = toks[c]?.text
      if (['constraint', 'not', 'default', 'identity', 'expression'].includes(next ?? '')) continue
      if (next === 'column') c++
      if (toks[c]?.text === 'if' && toks[c + 1]?.text === 'exists') c += 2
      const col = nameAt(toks, c)?.name
      add('drop-column', `DROP COLUMN ${col ? `${col} ` : ''}deletes column data from ${table}`)
    }
  }

  // UPDATE / DELETE in command position: at the start, opening a CTE or
  // subquery `(`, or after a CTE list `)`. Elsewhere (`ON DELETE`, `FOR
  // UPDATE`, `GRANT UPDATE`, `DO UPDATE`) they are not commands.
  for (let k = start; k < toks.length; k++) {
    const t = toks[k]!
    if (t.kind !== 'word' || (t.text !== 'update' && t.text !== 'delete')) continue
    const prev = toks[k - 1]?.text
    if (k !== start && prev !== '(' && prev !== ')') continue
    if (hasWhere(toks, k)) continue
    let j = k + 1
    if (t.text === 'delete' && toks[j]?.text === 'from') j++
    if (toks[j]?.text === 'only') j++
    const target = nameAt(toks, j)?.name ?? 'the table'
    add(
      'no-where',
      t.text === 'delete'
        ? `DELETE without WHERE removes every row of ${target}`
        : `UPDATE without WHERE changes every row of ${target}`
    )
  }
  return out
}

/** Every destructive part of `sql` (one or several statements). */
export function destructiveRisks(sql: string): Risk[] {
  return splitStatements(sql).flatMap((r) => risksOf(sql.slice(r.from, r.to)))
}

/**
 * Why `sql` would make a read-only session writable (`BEGIN READ WRITE`,
 * `SET default_transaction_read_only = off`, `set_config(…)`), or null.
 * Reading the settings (`SHOW`, `current_setting`) is fine.
 */
export function readOnlyEscape(sql: string): string | null {
  for (const r of splitStatements(sql)) {
    const toks = lex(sql.slice(r.from, r.to))
    for (let k = 0; k + 1 < toks.length; k++)
      if (toks[k]!.text === 'read' && toks[k + 1]!.text === 'write')
        return 'READ WRITE transactions are not allowed on a read-only connection'
    const setting = (t: Tok) =>
      (t.kind === 'word' || t.kind === 'quoted' || t.kind === 'string') &&
      t.text.toLowerCase().includes('transaction_read_only')
    const head = toks[0]?.text
    if ((head === 'set' || head === 'alter') && toks.some(setting))
      return 'The read-only setting cannot be changed on a read-only connection'
    if (toks.some((t) => t.text === 'set_config') && toks.some(setting))
      return 'The read-only setting cannot be changed on a read-only connection'
  }
  return null
}

/** Ask before running: something is risky and the preference (or a production tag) says so. */
export function needsConfirm(
  risks: readonly Risk[],
  opts: { confirmDestructive: boolean; production: boolean }
): boolean {
  return risks.length > 0 && (opts.confirmDestructive || opts.production)
}

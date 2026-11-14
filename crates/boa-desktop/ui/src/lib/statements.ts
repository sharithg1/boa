// Statements in a console script, DataGrip-style: the script splits on `;`
// outside strings, quoted names, comments and dollar quotes; each statement's
// range leaves out the blank lines and comments around it. Parenthesised
// queries (`(SELECT …)`, `(WITH …)`, `(VALUES …)`) inside a statement are
// recorded too, so ⌘↵ can offer "the whole statement or this subquery".

export type Range = { from: number; to: number }
export type Statement = Range & { subqueries: Range[] }

const QUERY_START = /^(select|with|values|table)\b/i

export function splitStatements(sql: string): Statement[] {
  const out: Statement[] = []
  const n = sql.length
  let first = -1 // first significant char of the current statement
  let last = -1 // end (exclusive) of its last significant token
  let subs: Range[] = []
  // Open parens: where the interior starts and whether it is a query.
  const parens: { at: number; query: boolean }[] = []
  const mark = (a: number, b: number) => {
    if (first < 0) first = a
    last = b
  }
  const flush = () => {
    if (first >= 0) out.push({ from: first, to: last, subqueries: subs })
    first = last = -1
    subs = []
    parens.length = 0
  }

  let i = 0
  while (i < n) {
    const c = sql[i]!
    const next = sql[i + 1]
    if (c === '-' && next === '-') {
      const e = sql.indexOf('\n', i)
      i = e < 0 ? n : e
    } else if (c === '/' && next === '*') {
      const e = sql.indexOf('*/', i + 2)
      i = e < 0 ? n : e + 2
    } else if (c === "'" || c === '"' || c === '`') {
      let j = i + 1
      while (j < n) {
        if (sql[j] === c) {
          if (sql[j + 1] === c) j += 2
          else {
            j++
            break
          }
        } else j++
      }
      mark(i, j)
      i = j
    } else if (c === '$' && /^\$[A-Za-z_]*\$/.test(sql.slice(i))) {
      const tag = /^\$[A-Za-z_]*\$/.exec(sql.slice(i))![0]
      const e = sql.indexOf(tag, i + tag.length)
      const j = e < 0 ? n : e + tag.length
      mark(i, j)
      i = j
    } else if (c === ';') {
      flush()
      i++
    } else if (/\s/.test(c)) {
      i++
    } else {
      if (c === '(') {
        let k = i + 1
        while (k < n && /\s/.test(sql[k]!)) k++
        parens.push({ at: k, query: QUERY_START.test(sql.slice(k, k + 7)) })
      } else if (c === ')') {
        const p = parens.pop()
        if (p?.query) {
          let e = i
          while (e > p.at && /\s/.test(sql[e - 1]!)) e--
          subs.push({ from: p.at, to: e })
        }
      }
      mark(i, i + 1)
      i++
    }
  }
  flush()
  return out
}

/**
 * What ⌘↵ can run at `pos`: the statement there, then any subqueries around
 * the caret from the outermost in. A caret on a statement's own line after
 * its `;` still counts as that statement; past the last statement, the last
 * one runs.
 */
export function statementsAt(sql: string, pos: number): Range[] {
  const all = splitStatements(sql)
  if (all.length === 0) return []
  let stmt = all.find((s, k) => {
    const prevEnd = k > 0 ? all[k - 1]!.to : -1
    const nextStart = all[k + 1]?.from ?? Number.POSITIVE_INFINITY
    if (pos >= s.from && pos <= s.to) return true
    // Between statements: belongs to this one if it is on its last line
    // (after the `;`), or anywhere after the previous one's line.
    if (pos > s.to && pos < nextStart) return !sql.slice(s.to, pos).includes('\n')
    if (pos < s.from && pos > prevEnd) return k === 0 || sql.slice(prevEnd, pos).includes('\n')
    return false
  })
  stmt ??= all[all.length - 1]!
  const subs = stmt.subqueries
    .filter((r) => pos >= r.from - 1 && pos <= r.to + 1)
    .sort((a, b) => b.to - b.from - (a.to - a.from))
  return [{ from: stmt.from, to: stmt.to }, ...subs]
}

export type TextMetrics = {
  charWidth: number
  lineHeight: number
  padLeft: number
  padTop: number
  tabSize: number
}

/**
 * SVG path outlining `range` as laid out in a monospace editor: one box per
 * line, joined into a single stepped outline. Content coordinates (not
 * adjusted for scrolling).
 */
export function rangePath(sql: string, { from, to }: Range, m: TextMetrics): string {
  const lines = sql.split('\n')
  const col = (text: string) => {
    let c = 0
    for (const ch of text) c = ch === '\t' ? (Math.floor(c / m.tabSize) + 1) * m.tabSize : c + 1
    return c
  }
  // Line / column of the range ends.
  let offset = 0
  const rows: { x0: number; x1: number; y: number }[] = []
  for (let li = 0; li < lines.length; li++) {
    const text = lines[li]!
    const start = offset
    const end = offset + text.length
    offset = end + 1
    if (end < from || start > to) continue
    const a = Math.max(from, start) - start
    const b = Math.min(to, end) - start
    const x0 = start >= from ? 0 : col(text.slice(0, a))
    const x1 = Math.max(col(text.slice(0, b)), x0 + 1)
    rows.push({ x0, x1, y: li })
  }
  if (rows.length === 0) return ''
  const pad = 3
  const X = (c: number, side: -1 | 1) => m.padLeft + c * m.charWidth + side * pad
  const Y = (row: number) => m.padTop + row * m.lineHeight
  const first = rows[0]!
  const lastRow = rows[rows.length - 1]!
  const pts: [number, number][] = [[X(first.x0, -1), Y(first.y)]]
  // Down the right side.
  for (let k = 0; k < rows.length; k++) {
    const r = rows[k]!
    pts.push([X(r.x1, 1), Y(r.y)], [X(r.x1, 1), Y(r.y + 1)])
  }
  // Back up the left side.
  pts.push([X(lastRow.x0, -1), Y(lastRow.y + 1)])
  for (let k = rows.length - 1; k >= 0; k--) {
    const r = rows[k]!
    pts.push([X(r.x0, -1), Y(r.y + 1)], [X(r.x0, -1), Y(r.y)])
  }
  return `M${pts.map(([x, y]) => `${x.toFixed(1)},${y.toFixed(1)}`).join('L')}Z`
}

/** One-line preview of a statement for the chooser. */
export function preview(sql: string, r: Range, max = 60): string {
  const flat = sql.slice(r.from, r.to).replace(/\s+/g, ' ').trim()
  return flat.length > max ? `${flat.slice(0, max - 1)}…` : flat
}

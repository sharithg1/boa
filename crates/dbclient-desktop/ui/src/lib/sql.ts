// SQL tokenizer for the console editor's highlighting. One pass, no parser:
// comments, strings, quoted identifiers, numbers, parameters, keywords, type
// names, function calls and, given the database's catalog, schema / table /
// column references. Tokens cover the input exactly (concatenating their
// text gives back the source), so the highlight layer lines up with the
// textarea above it character for character.

export type TokenKind =
  | 'keyword'
  | 'type'
  | 'function'
  | 'string'
  | 'number'
  | 'comment'
  | 'param'
  | 'schema'
  | 'table'
  | 'column'
  | 'operator'
  | 'plain'

export type Token = { kind: TokenKind; text: string }

/** Names the database knows, lower-cased, to colour identifiers that refer to them. */
export type Catalog = {
  schemas: ReadonlySet<string>
  tables: ReadonlySet<string>
  columns: ReadonlySet<string>
}

const KEYWORDS = new Set(
  `add all alter analyze and any as asc begin between by cascade case cast check collate column
  commit constraint create cross current_date current_time current_timestamp database default
  delete desc distinct do drop else end except exists explain false fetch filter first for foreign
  from full grant group having if ilike in index inner insert intersect into is join key last
  lateral left like limit materialized natural not null nulls offset on only or order outer over
  partition primary recursive references rename replace restrict returning revoke right rollback
  row rows schema select set show similar table temp temporary then to transaction true truncate
  union unique update using vacuum values view when where window with within`
    .split(/\s+/)
    .filter(Boolean)
)

const TYPES = new Set(
  `bigint bigserial bit bool boolean bytea char character date datetime decimal double float float4
  float8 int int2 int4 int8 integer interval json jsonb money numeric precision real serial
  smallint smallserial text time timestamp timestamptz tinyint unsigned uuid varchar zone`
    .split(/\s+/)
    .filter(Boolean)
)

const isIdentStart = (c: string) => /[A-Za-z_À-￿]/.test(c)
const isIdent = (c: string) => /[\w$À-￿]/.test(c)

export function tokenize(sql: string, catalog?: Catalog): Token[] {
  const out: Token[] = []
  const push = (kind: TokenKind, text: string) => {
    const last = out[out.length - 1]
    // Merge runs of the same kind to keep the DOM small.
    if (last && last.kind === kind && (kind === 'plain' || kind === 'operator')) last.text += text
    else out.push({ kind, text })
  }
  /** The previous non-blank token: after a `.` a word is a name, never a keyword. */
  const prevSignificant = () => {
    for (let k = out.length - 1; k >= 0; k--) {
      const t = out[k]!
      if (t.kind !== 'plain' || t.text.trim()) return t
    }
    return undefined
  }

  let i = 0
  const n = sql.length
  while (i < n) {
    const c = sql[i]!
    const next = sql[i + 1]

    // -- line comment
    if (c === '-' && next === '-') {
      const end = sql.indexOf('\n', i)
      const j = end < 0 ? n : end
      push('comment', sql.slice(i, j))
      i = j
      continue
    }
    // /* block comment */ (unterminated: to the end)
    if (c === '/' && next === '*') {
      const end = sql.indexOf('*/', i + 2)
      const j = end < 0 ? n : end + 2
      push('comment', sql.slice(i, j))
      i = j
      continue
    }
    // 'string' with '' escapes; E'…' and N'…' prefixes stay with the string.
    if (c === "'" || ((c === 'E' || c === 'e' || c === 'N' || c === 'n') && next === "'")) {
      let j = c === "'" ? i + 1 : i + 2
      while (j < n) {
        if (sql[j] === "'") {
          if (sql[j + 1] === "'") j += 2
          else {
            j++
            break
          }
        } else j++
      }
      push('string', sql.slice(i, j))
      i = j
      continue
    }
    // $$ dollar-quoted $$ / $tag$ … $tag$
    if (c === '$') {
      const tag = /^\$[A-Za-z_]*\$/.exec(sql.slice(i))
      if (tag) {
        const end = sql.indexOf(tag[0], i + tag[0].length)
        const j = end < 0 ? n : end + tag[0].length
        push('string', sql.slice(i, j))
        i = j
        continue
      }
      // $1 positional parameter
      const num = /^\$\d+/.exec(sql.slice(i))
      if (num) {
        push('param', num[0])
        i += num[0].length
        continue
      }
    }
    // :name / @name parameters (not :: casts)
    if ((c === ':' && next !== ':' && sql[i - 1] !== ':') || c === '@') {
      const m = /^[:@][A-Za-z_]\w*/.exec(sql.slice(i))
      if (m) {
        push('param', m[0])
        i += m[0].length
        continue
      }
    }
    // "quoted" or `quoted` identifier: coloured by what it names.
    if (c === '"' || c === '`') {
      const end = sql.indexOf(c, i + 1)
      const j = end < 0 ? n : end + 1
      const text = sql.slice(i, j)
      push(identKind(text.slice(1, end < 0 ? undefined : -1), sql, j, catalog), text)
      i = j
      continue
    }
    // numbers: 12, 1.5, .5, 1e10, 0x1f
    if (/\d/.test(c) || (c === '.' && next && /\d/.test(next))) {
      const m = /^(0x[0-9a-f]+|\d*\.?\d+(e[+-]?\d+)?)/i.exec(sql.slice(i))!
      // Part of an identifier like t1? Then it is plain text.
      if (i > 0 && isIdent(sql[i - 1]!)) {
        push('plain', m[0])
      } else push('number', m[0])
      i += m[0].length
      continue
    }
    // words
    if (isIdentStart(c)) {
      let j = i + 1
      while (j < n && isIdent(sql[j]!)) j++
      const word = sql.slice(i, j)
      const lower = word.toLowerCase()
      const prev = prevSignificant()
      const afterDot = prev?.text.endsWith('.') && prev.kind === 'operator'
      let kind: TokenKind
      if (!afterDot && KEYWORDS.has(lower)) kind = 'keyword'
      else if (!afterDot && TYPES.has(lower)) kind = 'type'
      else kind = identKind(word, sql, j, catalog)
      push(kind, word)
      i = j
      continue
    }
    if ('=<>!+-*/%|&^~.,;:()[]'.includes(c)) {
      push(c === '(' || c === ')' || c === ',' || c === ';' ? 'plain' : 'operator', c)
      i++
      continue
    }
    push('plain', c)
    i++
  }
  return out
}

/** Function call, schema, table, column, or plain, for an identifier ending at `end`. */
function identKind(
  name: string,
  sql: string,
  end: number,
  catalog: Catalog | undefined
): TokenKind {
  let k = end
  while (k < sql.length && (sql[k] === ' ' || sql[k] === '\t')) k++
  if (sql[k] === '(') return 'function'
  if (!catalog) return 'plain'
  const lower = name.toLowerCase()
  const dotted = sql[end] === '.'
  if (dotted && catalog.schemas.has(lower)) return 'schema'
  if (catalog.tables.has(lower)) return 'table'
  if (catalog.columns.has(lower)) return 'column'
  return 'plain'
}

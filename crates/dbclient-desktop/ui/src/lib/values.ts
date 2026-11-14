// Cell values in the result grid: what kind of value a column holds (from
// the type name the driver reports, e.g. Postgres' `int4`, `timestamptz`,
// `_text`), and a small JSON tokenizer / parser for json and jsonb cells
// and the JSON viewer. The parser keeps number and string literals as their
// source text, so 64-bit ids and exact decimals survive untouched.

export type ValueKind =
  | 'number'
  | 'boolean'
  | 'json'
  | 'temporal'
  | 'uuid'
  | 'binary'
  | 'array'
  | 'text'

export function valueKind(type: string): ValueKind {
  const t = type.toLowerCase().trim()
  if (t.startsWith('_') || t.endsWith('[]')) return 'array'
  if (t === 'json' || t === 'jsonb') return 'json'
  if (t === 'bool' || t === 'boolean') return 'boolean'
  if (t === 'uuid') return 'uuid'
  if (t === 'bytea' || /blob|binary/.test(t)) return 'binary'
  if (/^(date|time|timetz|timestamp|timestamptz|datetime\d?|smalldatetime|interval|year)\b/.test(t))
    return 'temporal'
  if (
    /^(int[248]?|integer|smallint|bigint|tinyint|mediumint|serial[248]?|bigserial|smallserial|oid|decimal|numeric|dec|real|double( precision)?|float[48]?|money)\b/.test(
      t
    )
  )
    return 'number'
  return 'text'
}

/** Text classes for each kind; JSON is coloured token by token instead. */
export const VALUE_CLASS: Record<ValueKind, string> = {
  number: 'text-sql-number',
  boolean: 'text-sql-keyword',
  temporal: 'text-sql-param',
  uuid: 'text-sql-table',
  binary: 'text-dim',
  array: 'text-soft',
  json: 'text-soft',
  text: 'text-soft'
}

// ── JSON tokens ─────────────────────────────────────────

export type JsonTokenKind = 'key' | 'string' | 'number' | 'literal' | 'punct' | 'plain'
export type JsonToken = { kind: JsonTokenKind; text: string }

export const JSON_CLASS: Record<JsonTokenKind, string> = {
  key: 'text-sql-column',
  string: 'text-sql-string',
  number: 'text-sql-number',
  literal: 'text-sql-keyword',
  punct: 'text-sql-operator',
  plain: ''
}

const STRING = /^"(?:[^"\\]|\\.)*"?/
const NUMBER = /^-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/
const LITERAL = /^(?:true|false|null)\b/

/**
 * Tokens covering `text` exactly (joined, they give the input back). Lenient:
 * anything that is not JSON comes out as plain text. A string is a key when
 * the next non-blank character is a colon.
 */
export function jsonTokens(text: string): JsonToken[] {
  const out: JsonToken[] = []
  let i = 0
  while (i < text.length) {
    const rest = text.slice(i)
    const c = text[i]!
    let m: RegExpExecArray | null
    if (c === '"' && (m = STRING.exec(rest))) {
      let k = i + m[0].length
      while (k < text.length && /\s/.test(text[k]!)) k++
      out.push({ kind: text[k] === ':' ? 'key' : 'string', text: m[0] })
      i += m[0].length
    } else if ((m = NUMBER.exec(rest))) {
      out.push({ kind: 'number', text: m[0] })
      i += m[0].length
    } else if ((m = LITERAL.exec(rest))) {
      out.push({ kind: 'literal', text: m[0] })
      i += m[0].length
    } else {
      const kind = '{}[]:,'.includes(c) ? 'punct' : 'plain'
      const last = out[out.length - 1]
      if (last?.kind === kind) last.text += c
      else out.push({ kind, text: c })
      i++
    }
  }
  return out
}

/** Pretty-printed with two-space indents, literals kept exactly as written. */
export function formatJson(text: string): string {
  let out = ''
  let depth = 0
  const nl = () => `\n${'  '.repeat(depth)}`
  const toks = jsonTokens(text).flatMap((t) =>
    // Split merged punctuation and drop whitespace between tokens.
    t.kind === 'punct' ? [...t.text].map((ch) => ({ kind: t.kind, text: ch })) : [t]
  )
  for (let k = 0; k < toks.length; k++) {
    const t = toks[k]!
    if (t.kind === 'plain') {
      if (t.text.trim()) out += t.text.trim()
      continue
    }
    const next = toks.slice(k + 1).find((x) => x.kind !== 'plain' || x.text.trim())
    if (t.text === '{' || t.text === '[') {
      const close = t.text === '{' ? '}' : ']'
      if (next?.text === close) {
        out += t.text + close
        k = toks.indexOf(next, k + 1)
        continue
      }
      depth++
      out += t.text + nl()
    } else if (t.text === '}' || t.text === ']') {
      depth = Math.max(0, depth - 1)
      out += nl() + t.text
    } else if (t.text === ',') out += `,${nl()}`
    else if (t.text === ':') out += ': '
    else out += t.text
  }
  return out
}

// ── JSON tree ───────────────────────────────────────────

export type JsonNode =
  | { t: 'object'; entries: [key: string, value: JsonNode][] }
  | { t: 'array'; items: JsonNode[] }
  /** Source text: strings keep their quotes and escapes. */
  | { t: 'string' | 'number' | 'literal'; src: string }

/** Parses JSON into a tree; throws a SyntaxError with the offset on bad input. */
export function parseJson(text: string): JsonNode {
  let i = 0
  const fail = (what: string): never => {
    throw new SyntaxError(`${what} at position ${i}`)
  }
  const ws = () => {
    while (i < text.length && /\s/.test(text[i]!)) i++
  }
  const value = (): JsonNode => {
    ws()
    const c = text[i]
    if (c === '{') {
      i++
      const entries: [string, JsonNode][] = []
      ws()
      if (text[i] === '}') {
        i++
        return { t: 'object', entries }
      }
      for (;;) {
        ws()
        const key = STRING.exec(text.slice(i))?.[0] ?? ''
        if (key.length < 2 || !key.endsWith('"')) fail('Expected a key')
        i += key.length
        ws()
        if (text[i] !== ':') fail('Expected ":"')
        i++
        entries.push([JSON.parse(key) as string, value()])
        ws()
        if (text[i] === ',') i++
        else if (text[i] === '}') {
          i++
          return { t: 'object', entries }
        } else fail('Expected "," or "}"')
      }
    }
    if (c === '[') {
      i++
      const items: JsonNode[] = []
      ws()
      if (text[i] === ']') {
        i++
        return { t: 'array', items }
      }
      for (;;) {
        items.push(value())
        ws()
        if (text[i] === ',') i++
        else if (text[i] === ']') {
          i++
          return { t: 'array', items }
        } else fail('Expected "," or "]"')
      }
    }
    const rest = text.slice(i)
    let m: RegExpExecArray | null
    if (c === '"' && (m = STRING.exec(rest)) && m[0].length > 1 && m[0].endsWith('"')) {
      i += m[0].length
      return { t: 'string', src: m[0] }
    }
    if ((m = NUMBER.exec(rest))) {
      i += m[0].length
      return { t: 'number', src: m[0] }
    }
    if ((m = LITERAL.exec(rest))) {
      i += m[0].length
      return { t: 'literal', src: m[0] }
    }
    return fail(i >= text.length ? 'Unexpected end' : 'Unexpected character')
  }
  const root = value()
  ws()
  if (i < text.length) fail('Unexpected text')
  return root
}

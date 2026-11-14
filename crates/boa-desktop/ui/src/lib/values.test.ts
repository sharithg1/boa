import { describe, expect, it } from 'vitest'
import { formatJson, jsonTokens, parseJson, valueKind } from './values'

describe('valueKind', () => {
  it('maps driver type names to kinds', () => {
    expect(valueKind('int4')).toBe('number')
    expect(valueKind('numeric')).toBe('number')
    expect(valueKind('interval')).toBe('temporal')
    expect(valueKind('point')).toBe('text')
    expect(valueKind('timestamptz')).toBe('temporal')
    expect(valueKind('jsonb')).toBe('json')
    expect(valueKind('bool')).toBe('boolean')
    expect(valueKind('uuid')).toBe('uuid')
    expect(valueKind('_int4')).toBe('array')
    expect(valueKind('text')).toBe('text')
  })
})

describe('jsonTokens', () => {
  it('covers the input and tells keys from strings', () => {
    const s = '{"a": "b", "n": [1, -2.5e3, true, null], "x": oops}'
    const toks = jsonTokens(s)
    expect(toks.map((t) => t.text).join('')).toBe(s)
    const kinds = toks.filter((t) => t.text.trim()).map((t) => `${t.kind}:${t.text.trim()}`)
    expect(kinds).toContain('key:"a"')
    expect(kinds).toContain('string:"b"')
    expect(kinds).toContain('number:-2.5e3')
    expect(kinds).toContain('literal:null')
    expect(kinds).toContain('plain:oops')
  })
})

describe('formatJson', () => {
  it('pretty-prints and keeps literals exact', () => {
    expect(formatJson('{"id":12345678901234567890,"a":[],"b":{"c":"x, {y}"}}')).toBe(
      '{\n  "id": 12345678901234567890,\n  "a": [],\n  "b": {\n    "c": "x, {y}"\n  }\n}'
    )
  })
})

describe('parseJson', () => {
  it('builds a tree with source literals', () => {
    expect(parseJson('{"a\\n": [1.50, "s"], "b": {}}')).toEqual({
      t: 'object',
      entries: [
        [
          'a\n',
          {
            t: 'array',
            items: [
              { t: 'number', src: '1.50' },
              { t: 'string', src: '"s"' }
            ]
          }
        ],
        ['b', { t: 'object', entries: [] }]
      ]
    })
  })
  it('rejects bad input', () => {
    expect(() => parseJson('{"a": }')).toThrow(SyntaxError)
    expect(() => parseJson('[1] x')).toThrow(SyntaxError)
  })
})

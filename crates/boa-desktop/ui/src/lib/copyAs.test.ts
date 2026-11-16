import { describe, expect, it } from 'vitest'
import {
  COPY_FORMATS,
  type CopySelection,
  copyAs,
  copyPlain,
  jsonValue,
  parsePgArray,
  sqlValue,
  unavailable
} from './copyAs'

const cols = [
  { name: 'id', type: 'int4' },
  { name: 'name', type: 'text' },
  { name: 'active', type: 'bool' },
  { name: 'meta', type: 'jsonb' }
]

const sel: CopySelection = {
  columns: cols,
  rows: [
    ['1', "O'Brien", 't', '{"a": 1, "big": 12345678901234567890}'],
    ['2', null, 'f', null]
  ]
}

describe('TSV / CSV', () => {
  it('writes TSV without and with a header, NULL as an empty field', () => {
    expect(copyAs('tsv', sel)).toBe(
      `1\tO'Brien\tt\t"{""a"": 1, ""big"": 12345678901234567890}"\n2\t\tf\t\n`
    )
    expect(copyAs('tsv-header', sel).split('\n')[0]).toBe('id\tname\tactive\tmeta')
  })

  it('quotes per RFC 4180 in CSV, header first, empty string as ""', () => {
    const s: CopySelection = {
      columns: [
        { name: 'a', type: 'text' },
        { name: 'b', type: 'text' }
      ],
      rows: [
        ['x,y', 'say "hi"'],
        ['', null],
        ['l1\nl2', 'plain']
      ]
    }
    expect(copyAs('csv', s)).toBe('a,b\n"x,y","say ""hi"""\n"",\n"l1\nl2",plain\n')
  })

  it('copies a single cell as its raw text with ⌘C, several as TSV', () => {
    const one: CopySelection = { columns: [cols[1]!], rows: [['a\tb']] }
    expect(copyPlain(one)).toBe('a\tb')
    expect(copyPlain({ columns: [cols[1]!], rows: [[null]] })).toBe('')
    expect(copyPlain({ columns: [cols[1]!], rows: [['a'], ['b']] })).toBe('a\nb\n')
  })
})

describe('Markdown', () => {
  it('writes a table with right-aligned numbers, escaping pipes and newlines', () => {
    const s: CopySelection = {
      columns: [
        { name: 'id', type: 'int8' },
        { name: 'note', type: 'text' }
      ],
      rows: [
        ['1', 'a | b'],
        ['2', 'line1\nline2'],
        ['3', null],
        ['4', 'back\\slash']
      ]
    }
    expect(copyAs('markdown', s)).toBe(
      [
        '| id | note |',
        '| ---: | --- |',
        '| 1 | a \\| b |',
        '| 2 | line1<br>line2 |',
        '| 3 | NULL |',
        '| 4 | back\\\\slash |',
        ''
      ].join('\n')
    )
  })
})

describe('JSON', () => {
  it('writes an array of objects keyed by column, typed values, JSON embedded', () => {
    expect(copyAs('json', sel)).toBe(
      [
        '[',
        '  {',
        '    "id": 1,',
        `    "name": "O'Brien",`,
        '    "active": true,',
        '    "meta": {',
        '      "a": 1,',
        '      "big": 12345678901234567890',
        '    }',
        '  },',
        '  {',
        '    "id": 2,',
        '    "name": null,',
        '    "active": false,',
        '    "meta": null',
        '  }',
        ']',
        ''
      ].join('\n')
    )
  })

  it('parses as JSON with the right types', () => {
    const v = JSON.parse(copyAs('json', sel)) as Record<string, unknown>[]
    expect(v[0]).toMatchObject({ id: 1, name: "O'Brien", active: true, meta: { a: 1 } })
    expect(v[1]).toEqual({ id: 2, name: null, active: false, meta: null })
  })

  it('writes an empty selection as []', () => {
    expect(copyAs('json', { columns: cols, rows: [] })).toBe('[]\n')
  })

  it('quotes numbers that are not JSON numbers, and invalid JSON', () => {
    expect(jsonValue('NaN', 'number')).toBe('"NaN"')
    expect(jsonValue('$1,200.00', 'number')).toBe('"$1,200.00"')
    expect(jsonValue('007', 'number')).toBe('"007"')
    expect(jsonValue('-1.5e3', 'number')).toBe('-1.5e3')
    expect(jsonValue('{not json', 'json')).toBe('"{not json"')
    expect(jsonValue('maybe', 'boolean')).toBe('"maybe"')
  })

  it('escapes strings and keeps bytea / dates as text', () => {
    expect(jsonValue('a"b\\c\nd', 'text')).toBe('"a\\"b\\\\c\\nd"')
    expect(jsonValue('\\x0102', 'binary')).toBe('"\\\\x0102"')
    expect(jsonValue('2024-01-02 03:04:05+00', 'temporal')).toBe('"2024-01-02 03:04:05+00"')
  })

  it('writes Postgres arrays as JSON arrays of the element type', () => {
    expect(JSON.parse(jsonValue('{1,2,NULL}', 'array', '_int4'))).toEqual([1, 2, null])
    expect(JSON.parse(jsonValue('{"a b","c\\"d",NULL,"NULL"}', 'array', '_text'))).toEqual([
      'a b',
      'c"d',
      null,
      'NULL'
    ])
    expect(JSON.parse(jsonValue('{{1,2},{3,4}}', 'array', 'int4[]'))).toEqual([
      [1, 2],
      [3, 4]
    ])
    expect(JSON.parse(jsonValue('{t,f}', 'array', '_bool'))).toEqual([true, false])
    expect(jsonValue('{}', 'array', '_int4')).toBe('[]')
    expect(jsonValue('[0:1]={1,2}', 'array', '_int4')).toBe('"[0:1]={1,2}"')
  })
})

describe('parsePgArray', () => {
  it('rejects text that is not an array literal', () => {
    expect(parsePgArray('{1,2')).toBeNull()
    expect(parsePgArray('{1,2}x')).toBeNull()
    expect(parsePgArray('1,2')).toBeNull()
    expect(parsePgArray('{"open}')).toBeNull()
  })
})

describe('SQL', () => {
  it('writes literals per type: numbers and booleans bare, quotes doubled', () => {
    expect(sqlValue(null, 'text')).toBe('NULL')
    expect(sqlValue('42', 'number')).toBe('42')
    expect(sqlValue('-0.5', 'number')).toBe('-0.5')
    expect(sqlValue('NaN', 'number')).toBe("'NaN'")
    expect(sqlValue('t', 'boolean')).toBe('TRUE')
    expect(sqlValue('f', 'boolean')).toBe('FALSE')
    expect(sqlValue("it's", 'text')).toBe("'it''s'")
    expect(sqlValue('{"a": 1}', 'json')).toBe(`'{"a": 1}'`)
    expect(sqlValue('{1,2}', 'array')).toBe("'{1,2}'")
    expect(sqlValue('\\x01', 'binary')).toBe("E'\\\\x01'")
  })

  it('writes one INSERT per row into MY_TABLE for console results', () => {
    expect(copyAs('sql-insert', sel)).toBe(
      `INSERT INTO MY_TABLE (id, name, active, meta) VALUES (1, 'O''Brien', TRUE, '{"a": 1, "big": 12345678901234567890}');\n` +
        'INSERT INTO MY_TABLE (id, name, active, meta) VALUES (2, NULL, FALSE, NULL);\n'
    )
  })

  it('uses the table name, quotes identifiers and writes DEFAULT for untouched new cells', () => {
    const s: CopySelection = {
      columns: [
        { name: 'id', type: 'int4' },
        { name: 'User Name', type: 'text' }
      ],
      rows: [[null, 'x']],
      table: 'public."Order"',
      isDefault: (r, c) => r === 0 && c === 0
    }
    expect(copyAs('sql-insert', s)).toBe(
      `INSERT INTO public."Order" (id, "User Name") VALUES (DEFAULT, 'x');\n`
    )
    // Other formats show the default as NULL.
    expect(copyAs('json', s)).toContain('"id": null')
  })

  it('writes UPDATEs by primary key, setting the other selected columns', () => {
    const s: CopySelection = {
      columns: [
        { name: 'id', type: 'int4' },
        { name: 'name', type: 'text' },
        { name: 'active', type: 'bool' }
      ],
      rows: [
        ['1', "O'Brien", 't'],
        ['2', null, 'f']
      ],
      table: 'public.users',
      key: {
        columns: [
          { name: 'id', type: 'int4' },
          { name: 'tenant', type: 'text' }
        ],
        rows: [
          ['1', 'acme'],
          ['2', null]
        ]
      }
    }
    expect(copyAs('sql-update', s)).toBe(
      `UPDATE public.users SET name = 'O''Brien', active = TRUE WHERE id = 1 AND tenant = 'acme';\n` +
        'UPDATE public.users SET name = NULL, active = FALSE WHERE id = 2 AND tenant IS NULL;\n'
    )
  })

  it('makes UPDATE unavailable without a table, a key or a non-key column', () => {
    const key = { columns: [{ name: 'id', type: 'int4' }], rows: [['1']] }
    expect(unavailable('sql-update', sel)).toBeTruthy()
    expect(unavailable('sql-update', { ...sel, key })).toBeTruthy()
    expect(unavailable('sql-update', { ...sel, table: 't' })).toBeTruthy()
    expect(unavailable('sql-update', { ...sel, table: 't', key })).toBeNull()
    expect(
      unavailable('sql-update', { columns: [cols[0]!], rows: [['1']], table: 't', key })
    ).toBeTruthy()
    expect(() => copyAs('sql-update', sel)).toThrow()
  })

  it('writes an IN list for a single column only', () => {
    const s: CopySelection = {
      columns: [{ name: 'name', type: 'text' }],
      rows: [['a'], ["b'c"], [null]]
    }
    expect(copyAs('sql-in', s)).toBe("'a', 'b''c', NULL\n")
    expect(
      copyAs('sql-in', { columns: [{ name: 'id', type: 'int8' }], rows: [['1'], ['2']] })
    ).toBe('1, 2\n')
    expect(unavailable('sql-in', s)).toBeNull()
    expect(unavailable('sql-in', sel)).toBe('Select a single column')
  })

  it('offers every format for a plain selection except UPDATE and IN', () => {
    expect(COPY_FORMATS.filter((f) => !unavailable(f.format, sel)).map((f) => f.format)).toEqual([
      'tsv',
      'tsv-header',
      'csv',
      'markdown',
      'json',
      'sql-insert'
    ])
  })
})

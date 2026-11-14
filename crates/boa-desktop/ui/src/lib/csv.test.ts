import { describe, expect, it } from 'vitest'
import { csvField, csvFileName, toCsv } from './csv'

describe('csvField', () => {
  it('leaves plain values unquoted', () => {
    expect(csvField('abc', ',')).toBe('abc')
    expect(csvField(' padded ', ',')).toBe(' padded ')
    expect(csvField('a;b', ',')).toBe('a;b')
  })

  it('quotes delimiters, quotes and line breaks, doubling quotes', () => {
    expect(csvField('a,b', ',')).toBe('"a,b"')
    expect(csvField('a;b', ';')).toBe('"a;b"')
    expect(csvField('a\tb', '\t')).toBe('"a\tb"')
    expect(csvField('say "hi"', ',')).toBe('"say ""hi"""')
    expect(csvField('l1\nl2', ',')).toBe('"l1\nl2"')
    expect(csvField('l1\r\nl2', ',')).toBe('"l1\r\nl2"')
  })

  it('writes NULL as an empty field and the empty string as ""', () => {
    expect(csvField(null, ',')).toBe('')
    expect(csvField('', ',')).toBe('""')
  })

  it('quotes the end-of-data marker only in a single column, like COPY', () => {
    expect(csvField('\\.', ',', true)).toBe('"\\."')
    expect(csvField('\\.', ',')).toBe('\\.')
  })
})

describe('toCsv', () => {
  const columns = ['id', 'note', 'e']
  const rows = [
    ['1', 'a,b', ''],
    ['2', null, null]
  ]

  it('writes a header row and \\n line ends', () => {
    expect(toCsv(columns, rows, { delimiter: ',', header: true })).toBe(
      'id,note,e\n1,"a,b",""\n2,,\n'
    )
  })

  it('can leave the header out and use another delimiter', () => {
    expect(toCsv(columns, rows, { delimiter: '\t', header: false })).toBe('1\ta,b\t""\n2\t\t\n')
    expect(toCsv(columns, rows, { delimiter: ';', header: false })).toBe('1;a,b;""\n2;;\n')
  })

  it('quotes header names like values', () => {
    expect(toCsv(['a,b', ''], [], { delimiter: ',', header: true })).toBe('"a,b",""\n')
  })

  it('is empty without rows or header', () => {
    expect(toCsv(columns, [], { delimiter: ',', header: false })).toBe('')
  })
})

describe('csvFileName', () => {
  it('names the file after the table read', () => {
    expect(csvFileName('select * from public.orders where x', 'console_1', ',')).toBe('orders.csv')
    expect(csvFileName('SELECT * FROM Orders', 'c', ',')).toBe('orders.csv')
    expect(csvFileName('select * from "Sales"."My Table"', 'c', '\t')).toBe('My Table.tsv')
  })

  it('falls back to the console name, made file-safe', () => {
    expect(csvFileName('select 1', 'console_1', ',')).toBe('console_1.csv')
    expect(csvFileName('select 1', 'a/b:c', ',')).toBe('a_b_c.csv')
    expect(csvFileName('select 1', '..', ',')).toBe('result.csv')
  })
})

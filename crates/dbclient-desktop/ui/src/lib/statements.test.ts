import { describe, expect, it } from 'vitest'
import { rangePath, splitStatements, statementsAt } from './statements'

const script = `-- Q1
SELECT 1;

-- Q2: subquery
SELECT a FROM t
WHERE id IN (SELECT id FROM u WHERE x = ';')
GROUP BY 1;
`
const text = (r: { from: number; to: number }) => script.slice(r.from, r.to)

describe('splitStatements', () => {
  it('splits on ; outside strings and leaves comments out', () => {
    const all = splitStatements(script)
    expect(all.map(text)).toEqual([
      'SELECT 1',
      "SELECT a FROM t\nWHERE id IN (SELECT id FROM u WHERE x = ';')\nGROUP BY 1"
    ])
    expect(all[1]!.subqueries.map(text)).toEqual(["SELECT id FROM u WHERE x = ';'"])
  })
  it('ignores ; in comments and dollar quotes', () => {
    expect(splitStatements('do $$ a; b $$; /* ; */ select 2 -- ;').length).toBe(2)
  })
})

describe('statementsAt', () => {
  it('picks the statement at the caret, then subqueries around it', () => {
    const inSub = script.indexOf('FROM u')
    expect(statementsAt(script, inSub).map(text)).toEqual([
      "SELECT a FROM t\nWHERE id IN (SELECT id FROM u WHERE x = ';')\nGROUP BY 1",
      "SELECT id FROM u WHERE x = ';'"
    ])
    // Right before the opening paren still offers the subquery.
    expect(statementsAt(script, script.indexOf('(SELECT')).length).toBe(2)
    expect(statementsAt(script, script.indexOf('GROUP')).length).toBe(1)
  })
  it('keeps the caret after ; on that statement, and blank lines go to the next', () => {
    const afterSemi = script.indexOf('SELECT 1;') + 'SELECT 1;'.length
    expect(statementsAt(script, afterSemi).map(text)).toEqual(['SELECT 1'])
    expect(text(statementsAt(script, afterSemi + 1)[0]!)).toMatch(/^SELECT a/)
    expect(text(statementsAt(script, script.indexOf('-- Q2'))[0]!)).toMatch(/^SELECT a/)
    expect(text(statementsAt(script, script.length)[0]!)).toMatch(/^SELECT a/)
  })
})

describe('rangePath', () => {
  it('outlines the lines of a range as one stepped shape', () => {
    const m = { charWidth: 10, lineHeight: 20, padLeft: 0, padTop: 0, tabSize: 4 }
    const d = rangePath('ab\nabcd', { from: 0, to: 7 }, m)
    expect(d.startsWith('M-3.0,0.0L23.0,0.0L23.0,20.0L43.0,20.0L43.0,40.0')).toBe(true)
    expect(d.endsWith('Z')).toBe(true)
  })
})

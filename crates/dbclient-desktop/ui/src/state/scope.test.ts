import { describe, expect, it } from 'vitest'
import { databaseSchemas, shownDatabases } from './store'
import type { Connection } from './types'

const conn = {
  id: 'c1',
  database: 'app',
  databases: ['postgres', 'app', 'analytics'].map((name) => ({ name, schemas: [] }))
} as unknown as Connection

describe('shownDatabases', () => {
  it("defaults to the connection's own database", () => {
    expect(shownDatabases({}, conn)).toEqual(['app'])
  })
  it("follows the pick in server order; '*' is everything", () => {
    expect(shownDatabases({ c1: ['analytics', 'postgres', 'gone'] }, conn)).toEqual([
      'postgres',
      'analytics'
    ])
    expect(shownDatabases({ c1: '*' }, conn)).toEqual(['postgres', 'app', 'analytics'])
    expect(shownDatabases({ c1: [] }, conn)).toEqual([])
  })
})

describe('databaseSchemas', () => {
  const names = ['audit', 'public', 'CleanClaims']
  it('defaults to public, else the first schema', () => {
    expect(databaseSchemas({}, 'c1', 'app', names)).toEqual(['public'])
    expect(databaseSchemas({}, 'c1', 'app', ['x', 'y'])).toEqual(['x'])
  })
  it('follows the pick per database', () => {
    const scopes = { 'c1/app': ['CleanClaims', 'audit'], 'c1/other': '*' as const }
    expect(databaseSchemas(scopes, 'c1', 'app', names)).toEqual(['audit', 'CleanClaims'])
    expect(databaseSchemas(scopes, 'c1', 'other', names)).toEqual(names)
  })
})

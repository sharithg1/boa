import { describe, expect, it } from 'vitest'
import { sampleConnections } from './mock'

describe('sample data', () => {
  it('has keys whose columns and foreign-key targets exist', () => {
    let fks = 0
    for (const c of sampleConnections())
      for (const db of c.databases)
        for (const s of db.schemas)
          for (const t of s.tables) {
            const names = new Set(t.columns.map((x) => x.name))
            for (const k of t.constraints ?? []) {
              for (const n of k.columns) expect(names, `${t.name}.${k.name}`).toContain(n)
              const r = k.references
              if (!r) continue
              fks++
              const target = db.schemas
                .find((x) => x.name === r.schema)
                ?.tables.find((x) => x.name === r.table)
              expect(target, k.name).toBeDefined()
              for (const n of r.columns) expect(target!.columns.map((x) => x.name)).toContain(n)
            }
            for (const i of t.indexes ?? [])
              for (const n of i.columns) expect(names, `${t.name}.${i.name}`).toContain(n)
          }
    expect(fks).toBeGreaterThan(2)
  })
})

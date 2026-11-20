import { describe, expect, it } from 'vitest'
import { elapsed } from './format'

describe('elapsed', () => {
  it('counts tenths of a second under a minute', () => {
    expect(elapsed(0)).toBe('0.0 s')
    expect(elapsed(449)).toBe('0.4 s')
    expect(elapsed(12_340)).toBe('12.3 s')
    expect(elapsed(59_999)).toBe('59.9 s')
  })
  it('switches to minutes and seconds', () => {
    expect(elapsed(60_000)).toBe('1 m 00 s')
    expect(elapsed(75_900)).toBe('1 m 15 s')
  })
  it('never goes negative', () => {
    expect(elapsed(-50)).toBe('0.0 s')
  })
})

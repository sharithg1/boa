import { describe, expect, it } from 'vitest'
import { CONNECTION_COLORS, connectionColor, isProduction } from './connTag'

describe('connectionColor', () => {
  it('prefers the picked colour, then the environment', () => {
    expect(connectionColor({})).toBeNull()
    expect(connectionColor({ environment: 'production' })).toBe(CONNECTION_COLORS.red)
    expect(connectionColor({ environment: 'production', color: 'purple' })).toBe(
      CONNECTION_COLORS.purple
    )
    expect(connectionColor({ color: 'green' })).toBe(CONNECTION_COLORS.green)
  })
  it('ignores unknown colour names (hand-edited config)', () => {
    expect(connectionColor({ color: 'toString' })).toBeNull()
    expect(connectionColor({ color: 'chartreuse', environment: 'staging' })).toBe(
      CONNECTION_COLORS.orange
    )
  })
  it('knows production', () => {
    expect(isProduction({ environment: 'production' })).toBe(true)
    expect(isProduction({ environment: 'staging', color: 'red' })).toBe(false)
  })
})

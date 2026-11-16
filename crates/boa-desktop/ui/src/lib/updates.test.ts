import { describe, expect, it } from 'vitest'
import { archLabel, releaseDateLabel, versionInfoText, type AppInfo } from './updates'

const info: AppInfo = {
  name: 'boa-desktop',
  version: '0.3.0',
  arch: 'aarch64',
  commit: 'abc123',
  commitDate: '2026-11-10T12:00:00Z',
  os: 'macOS 26.1'
}

describe('archLabel', () => {
  it('names the Mac architectures and passes others through', () => {
    expect(archLabel('aarch64')).toBe('Apple Silicon')
    expect(archLabel('x86_64')).toBe('Intel')
    expect(archLabel('riscv64')).toBe('riscv64')
  })
})

describe('versionInfoText', () => {
  it('lists version, commit, date and OS', () => {
    expect(versionInfoText(info)).toBe(
      'Boa 0.3.0 (Apple Silicon)\nCommit: abc123\nDate: 2026-11-10T12:00:00Z\nOS: macOS 26.1'
    )
  })

  it('leaves out an unknown commit and date', () => {
    expect(versionInfoText({ ...info, commit: '', commitDate: '' })).toBe(
      'Boa 0.3.0 (Apple Silicon)\nOS: macOS 26.1'
    )
  })
})

describe('releaseDateLabel', () => {
  const now = new Date('2026-11-15T12:00:00Z')
  it('says how long ago', () => {
    expect(releaseDateLabel('2026-11-15T08:00:00Z', now)).toBe('Nov 15, 2026 (today)')
    expect(releaseDateLabel('2026-11-14T08:00:00Z', now)).toBe('Nov 14, 2026 (1 day ago)')
    expect(releaseDateLabel('2026-11-10T12:00:00Z', now)).toBe('Nov 10, 2026 (5 days ago)')
  })

  it('is empty for a missing or bad date', () => {
    expect(releaseDateLabel('', now)).toBe('')
    expect(releaseDateLabel('not a date', now)).toBe('')
  })
})

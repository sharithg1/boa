import { describe, expect, it } from 'vitest'
import { cleanSsh, parseUrl, sshValid } from './ConnectionDialog'

describe('SSH tunnel settings', () => {
  it('saves nothing for a tunnel never filled in', () => {
    expect(cleanSsh(undefined)).toBeUndefined()
    expect(cleanSsh({ enabled: false, host: ' ', user: '', auth: 'agent' })).toBeUndefined()
  })
  it('trims and leaves out empty optional fields', () => {
    expect(
      cleanSsh({ enabled: true, host: ' bastion ', user: ' me ', auth: 'agent', identityFile: ' ' })
    ).toEqual({ enabled: true, host: 'bastion', user: 'me', auth: 'agent' })
    // Disabled but filled in: kept, so turning it back on restores it.
    expect(
      cleanSsh({
        enabled: false,
        host: 'b',
        port: 2222,
        user: '',
        auth: 'key-file',
        identityFile: '~/.ssh/k'
      })
    ).toEqual({
      enabled: false,
      host: 'b',
      port: 2222,
      user: '',
      auth: 'key-file',
      identityFile: '~/.ssh/k'
    })
  })
  it('requires a host, and a key file for key-file auth', () => {
    expect(sshValid(undefined)).toBe(true)
    expect(sshValid({ enabled: false, host: '', user: '', auth: 'key-file' })).toBe(true)
    expect(sshValid({ enabled: true, host: '', user: '', auth: 'agent' })).toBe(false)
    expect(sshValid({ enabled: true, host: 'b', user: '', auth: 'agent' })).toBe(true)
    expect(sshValid({ enabled: true, host: 'b', user: '', auth: 'key-file' })).toBe(false)
    expect(
      sshValid({ enabled: true, host: 'b', user: '', auth: 'key-file', identityFile: 'k' })
    ).toBe(true)
  })
})

describe('parseUrl', () => {
  it('reads every part of a postgres URL', () => {
    expect(
      parseUrl('postgresql://app%40x:p%3Ass@db.example.com:6543/my%20db?sslmode=require')
    ).toEqual({
      config: {
        host: 'db.example.com',
        port: 6543,
        user: 'app@x',
        database: 'my db',
        sslMode: 'require'
      },
      password: 'p:ss'
    })
  })
  it('defaults the port and maps verify-ca to verify-full', () => {
    expect(parseUrl('postgres://localhost?sslmode=verify-ca')).toEqual({
      config: { host: 'localhost', port: 5432, sslMode: 'verify-full' },
      password: undefined
    })
  })
  it('rejects other schemes and junk', () => {
    expect(parseUrl('mysql://x')).toBeNull()
    expect(parseUrl('not a url')).toBeNull()
  })
})

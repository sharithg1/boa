import { describe, expect, it } from 'vitest'
import { parseUrl } from './ConnectionDialog'

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

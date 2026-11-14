// Connection colour and environment tag: shown on the explorer row, the
// editor (a stripe over consoles and table tabs, the database title), the
// console toolbar and the status bar, so it is clear where a statement runs.

import type { Environment } from '@/state/types'

/** Palette: readable on both the light and the dark theme. */
export const CONNECTION_COLORS = {
  red: '#e5484d',
  orange: '#f76b15',
  yellow: '#d6a10b',
  green: '#30a46c',
  blue: '#0090ff',
  purple: '#8e4ec6',
  gray: '#8b8d98'
} as const

export type ConnectionColor = keyof typeof CONNECTION_COLORS

export const ENVIRONMENTS: [Environment, string][] = [
  ['development', 'Development'],
  ['test', 'Test'],
  ['staging', 'Staging'],
  ['production', 'Production']
]

/** Short badge text. */
export const ENV_BADGE: Record<Environment, string> = {
  development: 'DEV',
  test: 'TEST',
  staging: 'STAGE',
  production: 'PROD'
}

/** Colour an environment gets when none is picked. */
export const ENV_COLOR: Record<Environment, ConnectionColor> = {
  development: 'green',
  test: 'blue',
  staging: 'orange',
  production: 'red'
}

type Tagged = { environment?: Environment; color?: string }

export const isColor = (c: string | undefined): c is ConnectionColor =>
  !!c && Object.hasOwn(CONNECTION_COLORS, c)

/** The connection's colour (CSS), or null when it has neither a colour nor an environment. */
export function connectionColor(c: Tagged): string | null {
  if (isColor(c.color)) return CONNECTION_COLORS[c.color]
  return c.environment ? CONNECTION_COLORS[ENV_COLOR[c.environment]] : null
}

export const isProduction = (c: Tagged) => c.environment === 'production'

// Small formatting helpers for the chrome: counts, sizes, ages, engine
// names, and the fuzzy matcher of the ⌘K palette.
import type { Engine } from '@/state/types'

/** 1234 → "1.2k", 98120004 → "98M". */
export function compactCount(n: number): string {
  if (n < 1000) return String(n)
  const units: [number, string][] = [
    [1e9, 'B'],
    [1e6, 'M'],
    [1e3, 'k']
  ]
  for (const [v, u] of units) {
    if (n >= v) {
      const x = n / v
      return `${x < 10 ? x.toFixed(1).replace(/\.0$/, '') : Math.round(x)}${u}`
    }
  }
  return String(n)
}

export function bytes(n: number): string {
  const u = ['B', 'KB', 'MB', 'GB', 'TB']
  let i = 0
  let x = n
  while (x >= 1024 && i < u.length - 1) {
    x /= 1024
    i++
  }
  return `${x < 10 && i ? x.toFixed(1) : Math.round(x)} ${u[i]}`
}

/** "now", "5m", "3h", "2d". */
export function ago(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000))
  if (s < 45) return 'now'
  if (s < 3600) return `${Math.round(s / 60)}m`
  if (s < 86400) return `${Math.round(s / 3600)}h`
  return `${Math.round(s / 86400)}d`
}

export const ENGINE_LABEL: Record<Engine, string> = {
  postgres: 'PostgreSQL',
  mysql: 'MySQL',
  sqlite: 'SQLite'
}

/** Stable hue per name, for the coloured connection tiles. */
function tileHue(name: string): number {
  let h = 0
  for (const c of name) h = (h * 31 + c.charCodeAt(0)) >>> 0
  return h % 360
}

export function tileColor(name: string): string {
  return `oklch(var(--tile-l) var(--tile-c) ${tileHue(name)})`
}

// ── Fuzzy matching (⌘K palette) ───────────────────────────────────────────

export type FuzzyHit = { score: number; idxs: number[] }

/** Subsequence match; consecutive and word-start characters score higher. */
export function fuzzy(q: string, text: string): FuzzyHit | null {
  const t = text.toLowerCase()
  const query = q.toLowerCase()
  let i = 0
  let score = 0
  let prev = -2
  const idxs: number[] = []
  for (const ch of query) {
    const j = t.indexOf(ch, i)
    if (j < 0) return null
    idxs.push(j)
    score += j === prev + 1 ? 3 : 1
    if (j === 0 || /[\s._\-/]/.test(t[j - 1] ?? '')) score += 2
    prev = j
    i = j + 1
  }
  return { score: score - t.length * 0.01, idxs }
}

/** Splits `text` into runs for highlighting `idxs`. */
export function highlightRuns(text: string, idxs: number[]): { s: string; hit: boolean }[] {
  const set = new Set(idxs)
  const out: { s: string; hit: boolean }[] = []
  for (let i = 0; i < text.length; i++) {
    const hit = set.has(i)
    const last = out[out.length - 1]
    if (last && last.hit === hit) last.s += text[i]
    else out.push({ s: text[i]!, hit })
  }
  return out
}

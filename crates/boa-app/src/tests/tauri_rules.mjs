// Runs the Tauri app's own TypeScript parsers on the files the GPUI app
// wrote, to prove the Tauri app reads them as written. Used by
// tests/migration.rs when BOA_TAURI_UI_SRC is set; by hand:
//
//   BOA_TAURI_UI_SRC=<dbclient>/crates/boa-desktop/ui/src \
//     node tauri_rules.mjs <config dir> '["<connection id>", ...]'
//
// Needs node 22.18+ (TypeScript type stripping) and the source's
// node_modules (zustand). Nothing is written to the source tree. Prints one
// JSON object: what state/prefs.ts load(), state/session.ts parseSession()
// and state/csvExport.ts loadOptions() read from prefs.json, session.json
// and csv-export.json, and whether each read kept the file as written.

import { readFileSync } from 'node:fs'
import { register } from 'node:module'
import path from 'node:path'
import { pathToFileURL } from 'node:url'
import { isDeepStrictEqual } from 'node:util'

const [dir, idsJson] = process.argv.slice(2)
const SRC = process.env.BOA_TAURI_UI_SRC
if (!dir || !idsJson || !SRC) {
  console.error('usage: BOA_TAURI_UI_SRC=<ui/src> node tauri_rules.mjs <config dir> <ids json>')
  process.exit(2)
}

// Resolve the source's '@/…' alias and extension-less relative imports, as
// its vite config does.
const hooks = `
import { statSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
let SRC
export function initialize(data) { SRC = data.src }
function file(base) {
  for (const ext of ['', '.ts', '.tsx', '/index.ts']) {
    try { if (statSync(base + ext).isFile()) return base + ext } catch {}
  }
  return null
}
export async function resolve(spec, ctx, next) {
  let base = null
  if (spec.startsWith('@/')) base = path.join(SRC, spec.slice(2))
  else if ((spec.startsWith('./') || spec.startsWith('../')) && ctx.parentURL?.startsWith('file:')) {
    const parent = fileURLToPath(ctx.parentURL)
    if (parent.startsWith(SRC)) base = path.resolve(path.dirname(parent), spec)
  }
  const f = base && file(base)
  if (f) return { url: pathToFileURL(f).href, shortCircuit: true }
  return next(spec, ctx)
}
`
register('data:text/javascript,' + encodeURIComponent(hooks), { data: { src: SRC } })

const read = (name) => {
  try {
    return readFileSync(path.join(dir, name), 'utf8')
  } catch {
    return null
  }
}
const raw = {
  prefs: read('prefs.json'),
  session: read('session.json'),
  csv: read('csv-export.json')
}

// The Tauri app read these from localStorage under these keys.
const items = new Map()
if (raw.prefs !== null) items.set('boa.prefs.v2', raw.prefs)
if (raw.session !== null) items.set('boa.session.v1', raw.session)
if (raw.csv !== null) items.set('boa.csvExport', raw.csv)
globalThis.localStorage = {
  getItem: (k) => (items.has(k) ? items.get(k) : null),
  setItem: (k, v) => items.set(k, String(v)),
  removeItem: (k) => items.delete(k)
}

const load = (m) => import(pathToFileURL(path.join(SRC, m)).href)
const { usePrefs } = await load('state/prefs.ts')
const { parseSession } = await load('state/session.ts')
const { useCsvExport } = await load('state/csvExport.ts')

// JSON round trip: drops the undefined fields parseSession sets.
const plain = (v) => (v === undefined ? null : JSON.parse(JSON.stringify(v)))
const prefs = plain(usePrefs.getState())
const session = plain(parseSession(raw.session, JSON.parse(idsJson)))
const csv = plain(useCsvExport.getState().options)
const same = (parsed, text) => text !== null && isDeepStrictEqual(parsed, JSON.parse(text))

console.log(
  JSON.stringify({
    prefs,
    session,
    csv,
    prefsLossless: same(prefs, raw.prefs),
    sessionLossless: same(session, raw.session),
    csvLossless: same(csv, raw.csv)
  })
)

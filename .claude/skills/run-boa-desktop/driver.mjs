// Drive the desktop UI (mock backend) in headless Chrome. Reads one command
// per line from stdin; see SKILL.md for the command list.
//   node driver.mjs [url] < script
// Env: SHOTS (output dir), SCALE (device pixel ratio, default 2 = Retina),
// VIEWPORT (WxH, default 1400x900), KEEP_GOING=1 (log a failed step and go on,
// for tours run against older branches), HEADED=1.
import { mkdirSync, writeFileSync } from 'node:fs'
import { createInterface } from 'node:readline'
import { chromium } from 'playwright-core'

const url = process.argv[2] ?? 'http://localhost:5273/'
const out = process.env.SHOTS ?? 'shots'
mkdirSync(out, { recursive: true })

const size = (wxh) => {
  const [width, height] = wxh.split('x').map(Number)
  return { width, height }
}
const browser = await chromium.launch({ channel: 'chrome', headless: !process.env.HEADED })
const p = await browser.newPage({
  viewport: size(process.env.VIEWPORT ?? '1400x900'),
  deviceScaleFactor: Number(process.env.SCALE ?? 2)
})
// A tour run against an older branch hits steps that can't work there; fail them fast.
if (process.env.KEEP_GOING) p.setDefaultTimeout(5000)
// Page exceptions are printed as they happen, with the step that was running.
let current = '(load)'
let pageErrors = 0
p.on('pageerror', (e) => {
  pageErrors++
  console.log(`pageerror during \`${current}\`: ${e.message}`)
})
await p.goto(url)
// Other Vite apps live on nearby ports; don't drive the wrong one.
if ((await p.title()) !== 'Boa') {
  console.log(`FAILED: ${url} is not the Boa UI (title "${await p.title()}")`)
  await browser.close()
  process.exit(1)
}

const esc = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
// Editor tabs (TitleBar) carry data-tab-id; the right panel's and the output
// panel's tabs are role="tab" too.
const EDITOR_TAB = '[role="tab"][data-tab-id]'
const editorTab = (label) => p.locator(EDITOR_TAB, { hasText: label }).first()
// An explorer row: by node id when the name has a "/" (`prod-pg/analytics/public`,
// see `tree`), else the first row with exactly that label.
const node = (name) =>
  name.includes('/')
    ? p.locator(`[role="treeitem"][data-node-id="${name}"]`)
    : p
        .locator('[role="treeitem"]', {
          has: p.locator('span.min-w-0', { hasText: new RegExp(`^${esc(name)}$`) })
        })
        .first()
const setExpanded = async (name, open) => {
  const n = node(name)
  if ((await n.getAttribute('aria-expanded')) !== String(open))
    await n.locator('span[aria-hidden]').first().click()
}
const sql = () => p.getByLabel('SQL', { exact: true })
const grid = () => p.getByRole('grid', { name: 'Results' }).last()

// `ss name -- what it shows` lines collect here; written to $SHOTS/manifest.json.
const manifest = []
const shot = async (name, desc, el) => {
  const path = `${out}/${name}.png`
  await (el ?? p).screenshot({ path })
  manifest.push({ file: `${name}.png`, desc, viewport: p.viewportSize() })
  console.log('screenshot', path)
}
// Split `a b -- rest` into ([a, b], 'rest').
const described = (args) => {
  const i = args.indexOf('--')
  return i < 0 ? [args, ''] : [args.slice(0, i), args.slice(i + 1).join(' ')]
}

const commands = {
  // --- Database Explorer (sidebar tree) ---
  // Print the visible rows: indent, +/- (collapsed / expanded), label, meta, [node id].
  tree: async () => {
    const rows = await p.$$eval('[role="treeitem"]', (els) =>
      els.map((e) => {
        const depth = Math.round((Number.parseFloat(e.style.paddingLeft) - 6) / 16)
        const mark = { true: '- ', false: '+ ' }[e.getAttribute('aria-expanded')] ?? '  '
        const [label, meta] = [...e.querySelectorAll(':scope > span:not([aria-hidden])')].map(
          (s) => s.textContent
        )
        return `${'  '.repeat(depth)}${mark}${label}${meta ? `  (${meta})` : ''}  [${e.dataset.nodeId}]`
      })
    )
    console.log(rows.join('\n'))
  },
  // Expand a connection row (which connects it) and wait until it is connected.
  connect: async (...name) => {
    await setExpanded(name.join(' '), true)
    await node(name.join(' ')).locator('[data-conn-state="connected"]').waitFor()
  },
  // Click (select: a database becomes active, a table opens as a preview tab),
  // double-click (a table: pinned tab; others: toggle) or right-click (menu) a row.
  node: async (...name) => node(name.join(' ')).click(),
  'dblclick-node': async (...name) => node(name.join(' ')).dblclick(),
  'rclick-node': async (...name) => node(name.join(' ')).click({ button: 'right' }),
  'hover-node': async (...name) => node(name.join(' ')).hover(),
  // Expand / collapse a row (no-op when it already is).
  expand: async (...name) => setExpanded(name.join(' '), true),
  collapse: async (...name) => setExpanded(name.join(' '), false),

  // --- Query console ---
  // Replace the console's text (caret at the end). `\n` in the text = newline.
  sql: async () => {
    await sql().fill(rest.replaceAll('\\n', '\n'))
  },
  // Focus the editor and type at the caret, key by key (opens completion).
  // Spaces are kept as written (`sql-type  where id < 5` starts with a space).
  'sql-type': async () => {
    await sql().focus()
    await p.keyboard.type(rest)
  },
  // Put the caret at an offset in the console text (negative: from the end).
  caret: async (pos) => {
    await sql().focus()
    await sql().evaluate((el, n) => {
      const at = n < 0 ? el.value.length + n : n
      el.setSelectionRange(at, at)
    }, Number(pos))
  },
  // Run like ⌘↵ then Enter: the statement at the caret (the first choice when
  // the caret is in a subquery). Waits for the statement to finish.
  run: async () => {
    await sql().focus()
    await p.keyboard.press('Meta+Enter')
    if (await p.getByRole('listbox', { name: 'Statements' }).count()) await p.keyboard.press('Enter')
    else await p.keyboard.press('Meta+Enter')
    const busy = () =>
      p.evaluate(async () => {
        const { useStore } = await import('/src/state/store.ts')
        return Object.keys(useStore.getState().running).length > 0
      })
    while (await busy()) await p.waitForTimeout(50)
  },
  // The open completion list: one item per line, > = selected.
  completions: async () => {
    const items = await p.$$eval('[role="listbox"][aria-label="Completions"] [role="option"]', (els) =>
      els.map((e) => `${e.getAttribute('aria-selected') === 'true' ? '>' : ' '} ${e.innerText.replace(/\s+/g, ' ')}`)
    )
    console.log(items.length ? items.join('\n') : '(no completion list)')
  },

  // --- Results ---
  // Print the last result grid on screen: header, then up to `n` rows (default 10), tab-separated.
  grid: async (n = '10') => {
    const g = grid()
    const head = await g.locator('thead th').allInnerTexts()
    const rows = await g.locator('tbody tr').evaluateAll(
      (trs, n) => trs.slice(0, n).map((tr) => [...tr.children].map((td) => td.innerText.trim()).join('\t')),
      Number(n)
    )
    const total = await g.locator('tbody tr').count()
    console.log(head.map((h) => h.replace(/\s+/g, ' ').trim()).join('\t'))
    console.log(rows.join('\n'))
    console.log(`(${total} rows rendered)`)
  },
  // Click / double-click a grid cell by `row:col` (0-based, as data-cell).
  cell: async (rc) => grid().locator(`[data-cell="${rc}"]`).click(),
  'dblclick-cell': async (rc) => grid().locator(`[data-cell="${rc}"]`).dblclick(),
  // Switch the Database Output panel to its Output log and print it.
  output: async () => {
    await p.locator('section [role="tab"]', { hasText: /^Output$/ }).click()
    console.log(await p.locator('section > div').last().innerText())
  },
  // A tab of the Database Output panel (`Output`, or a result tab's title).
  'output-tab': async (...name) =>
    p.locator('section [role="tab"]', { hasText: name.join(' ') }).last().click(),

  // --- Right panel: Structure | Info | History ---
  right: async (name) => p.getByRole('tab', { name: new RegExp(`^${name}`, 'i') }).click(),

  // --- Editor tabs (title bar) ---
  'click-tab': async (...label) => editorTab(label.join(' ')).click(),
  'dblclick-tab': async (...label) => editorTab(label.join(' ')).dblclick(),
  'close-tab': async (...label) =>
    editorTab(label.join(' ')).getByRole('button', { name: /^Close tab/ }).click({ force: true }),
  // Editor tab strip: label, * = italic (preview), < = active.
  tabs: async () => {
    const t = await p.$$eval(EDITOR_TAB, (els) =>
      els.map((e) => {
        const s = e.querySelector('span.whitespace-nowrap')
        const italic = s && getComputedStyle(s).fontStyle === 'italic'
        return `${s?.textContent ?? '?'}${italic ? '*' : ''}${e.dataset.active === 'true' ? '<' : ''}`
      })
    )
    console.log('tabs', JSON.stringify(t))
  },

  // --- Generic ---
  // A button by its accessible name (icon buttons: their tooltip label), exact.
  'click-button': async (...name) =>
    p.getByRole('button', { name: name.join(' '), exact: true }).first().click(),
  // Hover a button by its accessible name (shows its tooltip).
  'hover-button': async (...name) =>
    p.getByRole('button', { name: name.join(' '), exact: true }).first().hover(),
  // An item of the open dropdown / context menu, by the start of its text.
  'menu-item': async (...text) =>
    p.getByRole('menuitem', { name: new RegExp(`^${esc(text.join(' '))}`) }).first().click(),
  // Fill a field by its label, placeholder or aria-label: `fill Host = db.example.com`.
  fill: async (...args) => {
    const [label, value = ''] = args.join(' ').split(/\s+=\s*/)
    const byLabel = p.getByLabel(label, { exact: true })
    await ((await byLabel.count()) ? byLabel : p.getByPlaceholder(label)).first().fill(value)
  },
  // Click / right-click / hover any element by its exact visible text.
  'click-text': async (...text) => p.getByText(text.join(' '), { exact: true }).first().click(),
  rclick: async (...text) =>
    p.getByText(text.join(' '), { exact: true }).first().click({ button: 'right' }),
  'hover-text': async (...text) => p.getByText(text.join(' '), { exact: true }).first().hover(),
  // Click the first match of a CSS selector.
  click: async (...selector) => p.locator(selector.join(' ')).first().click(),
  // Move the mouse out of the way (drops hover states and tooltips).
  unhover: async () => p.mouse.move(0, 0),
  // Type into the focused element; spaces are kept as written.
  type: async () => p.keyboard.type(rest),
  key: async (combo) => p.keyboard.press(combo),
  // An ⌥ shortcut (opt j, opt 1, opt ArrowDown). On a Mac the app only takes ⌥+key
  // when `key` is not plain ASCII (App.tsx isOptionShortcut), but Playwright's
  // Alt+j sends key "j"; send what a US layout composes instead.
  opt: async (k) => {
    const code = /^[a-z]$/i.test(k) ? `Key${k.toUpperCase()}` : /^\d$/.test(k) ? `Digit${k}` : k
    await p.evaluate((code) => {
      const key = code.startsWith('Arrow') ? code : '†'
      const init = { key, code, altKey: true, bubbles: true, cancelable: true }
      ;(document.activeElement ?? document.body).dispatchEvent(new KeyboardEvent('keydown', init))
    }, code)
  },
  // Drag a resize handle (role=separator) by its label, `d` px along its axis
  // (negative = left / up).
  'drag-separator': async (...args) => {
    const d = Number(args.pop())
    const b = await p.getByRole('separator', { name: args.join(' ') }).boundingBox()
    const x = b.x + b.width / 2
    const y = b.y + b.height / 2
    const horizontal = b.width > b.height
    await p.mouse.move(x, y)
    await p.mouse.down()
    await p.mouse.move(horizontal ? x : x + d, horizontal ? y + d : y, { steps: 8 })
    await p.mouse.up()
  },
  'dblclick-separator': async (...name) =>
    p.getByRole('separator', { name: name.join(' ') }).dblclick(),
  // Scroll the mouse wheel over an element (CSS selector): `scroll 800 [role=grid]`.
  scroll: async (dy, ...selector) => {
    await p.locator(selector.join(' ')).first().hover()
    await p.mouse.wheel(0, Number(dy))
  },
  // Every match's text (a selector may match several elements).
  text: async (...s) =>
    console.log((await p.locator(s.join(' ') || 'body').allInnerTexts()).join('\n').slice(0, 3000)),
  eval: async (...js) => console.log(await p.evaluate(js.join(' '))),
  // Run JS in the page with the app's own modules in scope (Vite dev serves /src):
  // `store` (state/store.ts), `prefs` (state/prefs.ts), `s` = store state.
  // e.g. `js store.useStore.setState({ settings: 'keyboard' })`
  js: async (...code) => {
    const r = await p.evaluate(async (code) => {
      const store = await import('/src/state/store.ts')
      const prefs = await import('/src/state/prefs.ts')
      const s = store.useStore.getState()
      return eval(code)
    }, code.join(' '))
    if (r !== undefined) console.log(typeof r === 'string' ? r : JSON.stringify(r))
  },
  wait: async (ms = '300') => p.waitForTimeout(Number(ms)),
  // `ss name [-- description]`; the description goes to manifest.json.
  ss: async (...args) => {
    const [[name = 'shot'], desc] = described(args)
    await shot(name, desc)
  },
  // Screenshot one element (first match of a CSS selector), e.g. `ss-el sidebar aside -- desc`.
  'ss-el': async (...args) => {
    const [[name, ...selector], desc] = described(args)
    await shot(name, desc, p.locator(selector.join(' ')).first())
  },
  // Resize the window, e.g. `viewport 1000x700`; bare `viewport`: back to the run's size.
  viewport: async (wxh) => p.setViewportSize(size(wxh || (process.env.VIEWPORT ?? '1400x900'))),
  // Load another URL, relative to the start URL (e.g. `goto /?overlay`).
  goto: async (to) => p.goto(new URL(to, url).href),
  // Start over: forget prefs (tree state, scopes, panel sizes), reload the start URL.
  fresh: async () => {
    await p.evaluate(() => localStorage.clear())
    await p.goto(url)
  },
  // `toasts off` hides toast notifications, `toasts on` shows them.
  toasts: async (state) =>
    p.evaluate((hide) => {
      let el = document.getElementById('driver-toasts')
      if (!el) {
        el = document.createElement('style')
        el.id = 'driver-toasts'
        document.head.append(el)
      }
      el.textContent = hide ? '[data-toast-kind] { display: none !important }' : ''
    }, state === 'off')
}

// The current line after the command and one space, verbatim (for typed text).
let rest = ''
let failed = false
for await (const line of createInterface({ input: process.stdin })) {
  const [cmd, ...args] = line.trim().split(/\s+/)
  if (!cmd || cmd.startsWith('#')) continue
  current = line.trim()
  rest = line.replace(/^\s*\S+ ?/, '')
  const fn = commands[cmd]
  if (!fn) {
    console.log(`unknown command: ${cmd} (${Object.keys(commands).join(', ')})`)
    failed = true
    continue
  }
  try {
    await fn(...args)
    // Let React settle before the next step.
    if (cmd !== 'wait') await p.waitForTimeout(150)
  } catch (e) {
    console.log(`FAILED ${line}: ${e.message.split('\n')[0]}`)
    failed = true
    if (!process.env.KEEP_GOING) break
  }
}
if (manifest.length) writeFileSync(`${out}/manifest.json`, `${JSON.stringify(manifest, null, 2)}\n`)
await browser.close()
// A page exception fails the run even when every command went through.
process.exit(failed || pageErrors > 0 ? 1 : 0)

// Write the README index of a UI catalog from the driver's manifest.json.
//   node index.mjs <catalog-dir>   (reads screenshots/manifest.json, writes README.md)
import { readFileSync, writeFileSync } from 'node:fs'
import { execSync } from 'node:child_process'

const dir = process.argv[2] ?? 'docs/ui-review'
const shots = JSON.parse(readFileSync(`${dir}/screenshots/manifest.json`, 'utf8'))
const git = (cmd) => {
  try {
    return execSync(`git ${cmd}`, { encoding: 'utf8' }).trim()
  } catch {
    return '?'
  }
}

const rows = shots
  .map(({ file, desc, viewport }) => {
    const size = viewport ? `${viewport.width}×${viewport.height}` : ''
    return `| [${file.replace(/\.png$/, '')}](screenshots/${file}) | ${desc || '—'} | ${size} |`
  })
  .join('\n')

writeFileSync(
  `${dir}/README.md`,
  `# DB Client UI catalog

Screenshots of every screen, panel, menu and dialog of the desktop app, for a
design review. Captured from branch \`${git('rev-parse --abbrev-ref HEAD')}\` at
\`${git('rev-parse --short HEAD')}\` on ${new Date().toISOString().slice(0, 10)}.

Regenerate with \`.claude/skills/run-dbclient-desktop/catalog.sh\` (the screen list is
\`.claude/skills/run-dbclient-desktop/tour.txt\`).

## How these were taken

- The real UI code (React, \`crates/dbclient-desktop/ui\`) running against its **mock
  backend** in headless Chrome, at 2× device pixel ratio. Connections, schemas,
  rows and timings are sample data.
- The app has a single dark theme.
- In the real app the window is a native macOS window (Tauri / WebKit): the
  title bar area has the traffic-light buttons at top left, and fonts render
  with WebKit, so expect small differences from these Chrome renders.
- Toast notifications are hidden except in \`47-toast\`.

**Not design issues** (mock / capture artifacts): generated sample rows, query
timings that change between runs, and results that ignore the statement's
\`where\` / \`join\` (the mock returns the table's sample rows).

## Review brief

Go through every screenshot and look for:

1. **Bad UI**: truncation and clipping, misalignment, inconsistent spacing or
   type sizes, low contrast, unclear icons, controls that look disabled (or
   don't), weak hierarchy, crowded rows, states that are hard to tell apart.
2. **Improvements**: clearer labels and copy, better defaults, missing
   affordances or feedback, layout changes that would help.
3. **Redundancies**: the same action or information shown in more than one
   place (e.g. explorer vs title bar vs right panel vs status bar vs settings),
   duplicate entry points, sections that repeat each other across screens.

Write findings to \`docs/ui-review/REVIEW.md\`, one section per screenshot (by
name), each finding with: what and where on the screen, why it's a problem,
the suggested change, and severity (high / medium / low). End with a
cross-screen section for redundancies and inconsistencies that span screens.
Annotated copies of screenshots, if any, go in \`docs/ui-review/annotated/\`
with the same file names.

## Screens

| Screen | What it shows | Window |
|---|---|---|
${rows}
`
)
console.log(`wrote ${dir}/README.md (${shots.length} screens)`)

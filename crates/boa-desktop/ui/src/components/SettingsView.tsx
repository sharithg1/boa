// Settings (linked from the sidebar / ⌘,), laid out like Cursor's: a
// full-height nav column (Back, search, pages with icons) beside a centred
// column of titled cards whose rows carry a label, a description and a
// right-aligned control (dropdown, switch or field). Covers the workspace
// until Back / Esc.
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { ArrowLeft, Keyboard, Plug, Search, Settings } from 'lucide-react'
import { connect, connectionString, disconnect, useStore, type SettingsPage } from '@/state/store'
import { resetPrefs, setPrefs, usePrefs, type Prefs } from '@/state/prefs'
import { ENGINE_LABEL } from '@/lib/format'
import { cn } from '@/lib/utils'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from '@/components/ui/select'
import { ConnStateDot, connMeta } from './Sidebar'
import { Tile } from './chrome'

const NAV: [SettingsPage, string, ReactNode][] = [
  ['general', 'General', <Settings key="general" />],
  ['connections', 'Connections', <Plug key="connections" />],
  ['keyboard', 'Keyboard', <Keyboard key="keyboard" />]
]

type Row = {
  id: keyof Prefs
  label: string
  desc?: string
  kind: 'choice' | 'switch' | 'select' | 'input'
  options?: [string, string][]
  ph?: string
}
const SECTIONS: { title: string; rows: Row[] }[] = [
  {
    title: 'Sidebar and search',
    rows: [
      {
        id: 'sidebarRowCounts',
        kind: 'switch',
        label: 'Row counts in the sidebar',
        desc: "Each table's approximate row count, from the planner's statistics."
      },
      { id: 'sidebarViews', kind: 'switch', label: 'Show views in the sidebar' },
      {
        id: 'recentCount',
        kind: 'select',
        label: 'Recent tables in search',
        options: [
          ['3', '3'],
          ['5', '5'],
          ['8', '8'],
          ['10', '10']
        ]
      }
    ]
  },
  {
    title: 'Queries',
    rows: [
      {
        id: 'rowLimit',
        kind: 'select',
        label: 'Rows loaded when opening a table',
        options: [
          ['100', '100'],
          ['500', '500'],
          ['1000', '1,000'],
          ['5000', '5,000']
        ]
      },
      {
        id: 'confirmDestructive',
        kind: 'switch',
        label: 'Confirm destructive statements',
        desc: 'Ask before running UPDATE or DELETE without a WHERE clause, DROP, TRUNCATE or DROP COLUMN. Connections tagged Production always ask.'
      },
      {
        id: 'recordHistory',
        kind: 'switch',
        label: 'Save query history',
        desc: 'Keep every statement you run, per connection, for Query History (⌘⌥E).'
      }
    ]
  }
]

const KEYS: [string, string][] = [
  ['⌘K', 'Search tables'],
  ['⌘T / ⌥N', 'New query'],
  ['⌘↵', 'Run query'],
  ['⌘⌥E', 'Query history'],
  ['⌘W', 'Close tab'],
  ['⌥1 … ⌥9', 'Go to tab'],
  ['⌥↑ / ⌥↓', 'Previous / next database'],
  ['⌥B', 'Toggle sidebar'],
  ['⌥E', 'Toggle right panel'],
  ['⌘,', 'Settings']
]

export function SettingsView() {
  const page = useStore((s) => s.settings)
  const [q, setQ] = useState('')
  const [saved, setSaved] = useState(false)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const prefs = usePrefs()
  const searchRef = useRef<HTMLInputElement>(null)
  const close = () => {
    useStore.setState({ settings: null })
  }
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.code === 'KeyF') {
        e.preventDefault()
        searchRef.current?.focus()
      } else if (
        e.key === 'Escape' &&
        !e.defaultPrevented &&
        !document.querySelector('[role=listbox]')
      ) {
        const t = e.target as HTMLElement
        if (t.tagName === 'INPUT' && (t as HTMLInputElement).value) return
        useStore.setState({ settings: null })
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])
  if (!page) return null
  const set = (patch: Partial<Prefs>) => {
    setPrefs(patch)
    setSaved(true)
    if (timer.current) clearTimeout(timer.current)
    timer.current = setTimeout(() => setSaved(false), 1600)
  }
  const query = q.trim().toLowerCase()
  const sections = SECTIONS.map((sec) => ({
    ...sec,
    rows: sec.rows.filter(
      (r) => !query || `${r.label} ${r.desc ?? ''} ${sec.title}`.toLowerCase().includes(query)
    )
  })).filter((s) => s.rows.length)
  const showGeneral = page === 'general' || !!query
  const title = showGeneral ? 'General' : NAV.find((n) => n[0] === page)![1]
  return (
    <div className="absolute inset-0 z-40 flex bg-background" role="region" aria-label="Settings">
      <nav className="flex w-[260px] shrink-0 flex-col border-r border-border bg-sidebar text-sidebar-foreground">
        <div data-tauri-drag-region="deep" className="h-10 shrink-0 select-none" />
        <div className="flex min-h-0 flex-1 flex-col gap-px px-2">
          <button
            type="button"
            onClick={close}
            title="Back (esc)"
            className={cn(navItem, 'text-quiet')}
          >
            <ArrowLeft />
            Back
          </button>
          <div className="my-2 flex h-[30px] items-center gap-2 rounded-md border border-border bg-field px-2 focus-within:border-border-strong">
            <Search className="size-4 shrink-0 text-faint" />
            <input
              ref={searchRef}
              value={q}
              onChange={(e) => {
                setQ(e.target.value)
                if (e.target.value) useStore.setState({ settings: 'general' })
              }}
              placeholder="Search Settings"
              aria-label="Search settings"
              className="min-w-0 flex-1 border-none bg-transparent text-base text-foreground outline-none placeholder:text-faint"
            />
          </div>
          {NAV.map(([id, label, icon]) => (
            <button
              key={id}
              type="button"
              aria-current={page === id && !query ? 'page' : undefined}
              onClick={() => {
                setQ('')
                useStore.setState({ settings: id })
              }}
              className={cn(
                navItem,
                page === id && !query && 'bg-sidebar-accent text-foreground hover:bg-sidebar-accent'
              )}
            >
              {icon}
              {label}
            </button>
          ))}
        </div>
        <div className="flex h-12 shrink-0 items-center px-4 text-xs text-faint">
          {saved ? 'Saved' : 'Changes save automatically'}
        </div>
      </nav>
      <div className="flex min-w-0 flex-1 flex-col">
        <div data-tauri-drag-region="deep" className="h-10 shrink-0 select-none" />
        <div className="min-h-0 flex-1 overflow-y-auto px-8 pb-16">
          <div className="mx-auto max-w-[780px] pt-4">
            <h1 className="px-2 text-lg font-medium text-foreground">{title}</h1>
            {showGeneral &&
              sections.map((sec) => (
                <Section key={sec.title} title={sec.title}>
                  <Card>
                    {sec.rows.map((r) => (
                      <SettingRow
                        key={r.id}
                        row={r}
                        value={prefs[r.id]}
                        onChange={(v) => set({ [r.id]: v } as Partial<Prefs>)}
                      />
                    ))}
                  </Card>
                </Section>
              ))}
            {showGeneral && !sections.length && (
              <p className="mt-6 px-2 text-sm text-faint">No settings match.</p>
            )}
            {showGeneral && !query && (
              <button
                type="button"
                onClick={() => {
                  resetPrefs()
                  setSaved(true)
                }}
                className={cn(btn, 'mt-6')}
              >
                Reset to defaults
              </button>
            )}
            {!showGeneral && page === 'connections' && <ConnectionsPage />}
            {!showGeneral && page === 'keyboard' && (
              <Section title="Shortcuts work everywhere, including inside the editor">
                <Card>
                  {KEYS.map(([k, label]) => (
                    <div key={k} className="flex items-center px-1 py-2.5 text-sm text-body">
                      {label}
                      <span className="flex-1" />
                      <span className="font-mono text-xs text-muted-foreground">{k}</span>
                    </div>
                  ))}
                </Card>
              </Section>
            )}
          </div>
        </div>
      </div>
    </div>
  )
}

const navItem =
  'flex h-[30px] cursor-pointer items-center gap-2.5 rounded-md px-2 text-left text-base text-soft hover:bg-hover hover:text-foreground [&_svg]:size-4 [&_svg]:shrink-0 [&_svg]:text-quiet'

const btn =
  'h-7 cursor-pointer rounded-md border border-border px-2.5 text-sm text-soft hover:bg-hover hover:text-foreground'

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="mt-6">
      <h2 className="mb-2 px-2 text-sm text-muted-foreground">{title}</h2>
      {children}
    </section>
  )
}

/** Rows inset from the card edge, split by hairlines that stop at the inset (as in Cursor). */
function Card({ children }: { children: ReactNode }) {
  return (
    <div className="divide-y divide-foreground/[0.07] rounded-lg bg-chrome px-3 dark:bg-card">
      {children}
    </div>
  )
}

function SettingRow({
  row,
  value,
  onChange
}: {
  row: Row
  value: unknown
  onChange: (v: unknown) => void
}) {
  return (
    <div className="flex min-h-[60px] items-center gap-6 px-1 py-2.5">
      <div className="min-w-0 flex-1">
        <div className="text-sm text-foreground">{row.label}</div>
        {row.desc && <div className="mt-0.5 text-sm text-muted-foreground">{row.desc}</div>}
      </div>
      {(row.kind === 'select' || row.kind === 'choice') && (
        <Select
          value={String(value)}
          onValueChange={(v) => onChange(row.kind === 'select' ? Number(v) : v)}
        >
          <SelectTrigger
            aria-label={row.label}
            className="h-7 gap-1 rounded-md border-border bg-transparent px-2 text-sm text-foreground shadow-none dark:bg-transparent dark:hover:bg-hover [&_svg]:size-3.5"
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent position="popper" align="end">
            {row.options!.map(([k, l]) => (
              <SelectItem key={k} value={k}>
                {l}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
      {row.kind === 'switch' && (
        <Switch
          checked={value === true}
          onCheckedChange={(v) => onChange(v)}
          aria-label={row.label}
          className="h-5 w-9 data-[state=checked]:bg-status-ok dark:data-[state=unchecked]:bg-input [&>span]:size-4 [&>span]:bg-white [&>span]:data-[state=checked]:translate-x-[17px] [&>span]:data-[state=unchecked]:translate-x-px dark:[&>span]:data-[state=checked]:bg-white dark:[&>span]:data-[state=unchecked]:bg-white"
        />
      )}
      {row.kind === 'input' && (
        <Input
          aria-label={row.label}
          value={String(value)}
          placeholder={row.ph}
          spellCheck={false}
          onChange={(e) => onChange(e.target.value)}
          className="h-7 w-[220px] rounded-md border-border bg-transparent px-2 font-mono text-sm shadow-none dark:bg-transparent"
        />
      )}
    </div>
  )
}

function ConnectionsPage() {
  const order = useStore((s) => s.connectionOrder)
  const connections = useStore((s) => s.connections)
  return (
    <Section title="Saved connections. Credentials are stored in the local config file">
      <Card>
        {order.map((id) => {
          const c = connections[id]
          if (!c) return null
          return (
            <div key={id} className="flex items-center gap-3 px-1 py-3">
              <Tile name={c.name} />
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2 text-sm font-medium text-foreground">
                  {c.name}
                  <ConnStateDot conn={c} />
                </div>
                <div className="truncate font-mono text-xs text-faint">
                  {ENGINE_LABEL[c.engine]} · {connectionString(c)} · {connMeta(c)}
                </div>
              </div>
              {c.state === 'connected' ? (
                <button type="button" className={btn} onClick={() => disconnect(id)}>
                  Disconnect
                </button>
              ) : (
                <button
                  type="button"
                  className={btn}
                  disabled={c.state === 'connecting'}
                  onClick={() => connect(id)}
                >
                  {c.state === 'reconnecting' ? 'Retry now' : 'Connect'}
                </button>
              )}
            </div>
          )
        })}
        {order.length === 0 && <div className="px-1 py-3 text-sm text-faint">No connections.</div>}
      </Card>
    </Section>
  )
}

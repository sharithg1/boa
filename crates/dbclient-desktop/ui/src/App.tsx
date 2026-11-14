import { useEffect } from 'react'
import { BottomPanel } from './components/BottomPanel'
import { CommandPalette } from './components/CommandPalette'
import { ConnectionDialog, PasswordPrompt } from './components/ConnectionDialog'
import { QueryHistory } from './components/QueryHistory'
import { RightPanel } from './components/RightPanel'
import { SettingsView } from './components/SettingsView'
import { Sidebar, newQueryTarget } from './components/Sidebar'
import { StatusBar } from './components/StatusBar'
import { DiscardEditsDialog } from './components/TableEdits'
import { TitleBar } from './components/TitleBar'
import { Toasts } from './components/Toasts'
import { Workspace } from './components/Workspace'
import {
  activateTabIndex,
  closeTab,
  cycleDatabase,
  dbKey,
  loadConnections,
  newQueryTab,
  useStore
} from './state/store'
import { startSession } from './state/session'
import { toggleHistory } from './state/history'

export function App() {
  const sidebarOpen = useStore((s) => s.sidebarOpen)
  const rightPanelOpen = useStore((s) => s.rightPanelOpen)
  const bottomOpen = useStore((s) => s.bottomOpen)

  useShortcuts()
  useEffect(() => {
    // The last session's tabs belong to saved connections: restore them after.
    void loadConnections().then((ok) => {
      if (ok) startSession()
    })
  }, [])

  return (
    // DataGrip-style tool windows: Database Explorer on the left, the editor
    // (tabs over the active console or table) in the middle and the right
    // panel, above a full-width Database Output panel and status bar.
    <div className="relative flex h-full w-full flex-col overflow-hidden bg-background">
      <div className="flex min-h-0 flex-1">
        {sidebarOpen && <Sidebar />}
        <div className="flex min-w-0 flex-1 flex-col">
          <TitleBar />
          <Workspace />
        </div>
        {rightPanelOpen && <RightPanel />}
      </div>
      {bottomOpen && <BottomPanel />}
      <StatusBar />
      <SettingsView />
      <CommandPalette />
      <QueryHistory />
      <ConnectionDialog />
      <PasswordPrompt />
      <DiscardEditsDialog />
      <Toasts />
    </div>
  )
}

const isMac = () => navigator.platform.toLowerCase().includes('mac')

/**
 * ⌥-shortcuts must not eat characters a keyboard layout composes with Option
 * (German ⌥5 = "[", ⌥L = "@"…), which the editor needs. On US-style layouts
 * ⌥ produces non-ASCII symbols or dead keys, so only those are taken.
 * Elsewhere Alt does not compose, so Alt+key is always a shortcut.
 */
function isOptionShortcut(e: KeyboardEvent): boolean {
  if (!e.altKey || e.metaKey || e.ctrlKey) return false
  if (!isMac() || e.key === 'Dead' || e.key.startsWith('Arrow')) return true
  return !(e.key.length === 1 && e.key >= ' ' && e.key <= '~')
}

/**
 * Cmd/Ctrl+K search, +T new query, +W close tab, +O new connection,
 * +, settings, +⌥E query history; ⌥↑↓ previous / next database, ⌥1-9 tab, ⌥N new query,
 * ⌥B sidebar, ⌥E right panel, ⌥J Database Output.
 */
function useShortcuts() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const state = useStore.getState()
      const mod = isMac() ? e.metaKey : e.ctrlKey && e.shiftKey
      const overlay = !!document.querySelector('[role=dialog],[role=alertdialog]')
      if (mod && (e.code === 'KeyK' || e.code === 'KeyP')) {
        e.preventDefault()
        useStore.setState({ paletteOpen: !state.paletteOpen, settings: null })
        return
      }
      if (mod && e.altKey && e.code === 'KeyE') {
        e.preventDefault()
        e.stopPropagation()
        useStore.setState({ paletteOpen: false, settings: null })
        toggleHistory()
        return
      }
      if (mod && e.code === 'KeyO' && !e.shiftKey && !overlay) {
        e.preventDefault()
        useStore.setState({ connectionDialog: {} })
        return
      }
      if (mod && e.key === ',') {
        e.preventDefault()
        useStore.setState({ settings: state.settings ? null : 'general' })
        return
      }
      if (overlay || state.settings) return
      const { active, workspaces } = state
      if (mod) {
        if (!active) return
        const key = e.key.toLowerCase()
        if (key === 't') {
          e.preventDefault()
          newQueryTab(active.connectionId, active.database)
        } else if (key === 'w') {
          const k = dbKey(active.connectionId, active.database)
          const id = workspaces[k]?.activeTab
          if (!id) return
          e.preventDefault()
          closeTab(k, id)
        }
        return
      }
      if (!isOptionShortcut(e)) return
      const c = e.code
      let action: (() => void) | null = null
      if (c === 'ArrowDown' || c === 'ArrowUp')
        action = () => cycleDatabase(c === 'ArrowDown' ? 1 : -1)
      else if (c === 'KeyB') action = () => useStore.setState({ sidebarOpen: !state.sidebarOpen })
      else if (c === 'KeyJ') action = () => useStore.setState({ bottomOpen: !state.bottomOpen })
      else if (c === 'KeyE')
        action = () => useStore.setState({ rightPanelOpen: !state.rightPanelOpen })
      else if (active && /^Digit[1-9]$/.test(c))
        action = () => activateTabIndex(Number(c.slice(5)) - 1)
      else if (c === 'KeyN')
        action = () => {
          const t = newQueryTarget()
          if (t) newQueryTab(t.connectionId, t.database)
        }
      if (!action) return
      e.preventDefault()
      e.stopPropagation()
      action()
    }
    // Capture phase: the editor's textarea would otherwise swallow the keystroke.
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [])
}

import { useEffect } from 'react'
import { CommandPalette } from './components/CommandPalette'
import { RightPanel } from './components/RightPanel'
import { SettingsView } from './components/SettingsView'
import { Sidebar, newQueryTarget } from './components/Sidebar'
import { StatusBar } from './components/StatusBar'
import { TitleBar } from './components/TitleBar'
import { Toasts } from './components/Toasts'
import { Workspace } from './components/Workspace'
import {
  activateTabIndex,
  closeTab,
  cycleDatabase,
  dbKey,
  newQueryTab,
  pushToast,
  useStore
} from './state/store'

export function App() {
  const sidebarOpen = useStore((s) => s.sidebarOpen)
  const rightPanelOpen = useStore((s) => s.rightPanelOpen)

  useShortcuts()

  return (
    // Cursor-style columns: the sidebar and the right panel run the full
    // window height with their own header rows; the main column stacks its
    // header (tabs), the workspace and a footer line.
    <div className="relative flex h-full w-full overflow-hidden bg-background">
      {sidebarOpen && <Sidebar />}
      <div className="flex min-w-0 flex-1 flex-col">
        <TitleBar />
        <Workspace />
        <StatusBar />
      </div>
      {rightPanelOpen && <RightPanel />}
      <SettingsView />
      <CommandPalette />
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
 * +, settings; ⌥↑↓ previous / next database, ⌥1-9 tab, ⌥N new query,
 * ⌥B sidebar, ⌥E right panel.
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
      if (mod && e.code === 'KeyO' && !e.shiftKey && !overlay) {
        e.preventDefault()
        pushToast({ kind: 'info', title: 'New connection: not built yet' }, 2400)
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

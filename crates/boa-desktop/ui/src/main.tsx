import { createRoot } from 'react-dom/client'
// Bundled webfonts (offline-safe in the Tauri app): Geist for the chrome,
// Geist Mono for SQL, values and mono labels.
import '@fontsource-variable/geist'
import '@fontsource-variable/geist-mono'
import './assets/main.css'
import { isTauri } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { App } from './App'
import { AboutWindow } from './components/AboutWindow'
import { TooltipProvider } from './components/ui/tooltip'

// The About window (src/updater.rs) loads this same bundle; `?about` previews
// it in a browser.
const isAbout = isTauri()
  ? getCurrentWindow().label === 'about'
  : new URLSearchParams(location.search).has('about')

createRoot(document.getElementById('root')!).render(
  // One provider for every tooltip (icon buttons); a short delay so sweeping
  // across the tab bar does not flash labels.
  <TooltipProvider delayDuration={400}>{isAbout ? <AboutWindow /> : <App />}</TooltipProvider>
)

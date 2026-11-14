import { createRoot } from 'react-dom/client'
// Bundled webfonts (offline-safe in the Tauri app): Geist for the chrome,
// Geist Mono for SQL, values and mono labels.
import '@fontsource-variable/geist'
import '@fontsource-variable/geist-mono'
import './assets/main.css'
import { App } from './App'
import { TooltipProvider } from './components/ui/tooltip'

createRoot(document.getElementById('root')!).render(
  // One provider for every tooltip (icon buttons); a short delay so sweeping
  // across the tab bar does not flash labels.
  <TooltipProvider delayDuration={400}>
    <App />
  </TooltipProvider>
)

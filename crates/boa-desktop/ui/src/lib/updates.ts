// In-app updates and version info (src/updater.rs on the Rust side). Used
// by the About window and the status bar's "Restart to update" pill.
// Outside Tauri (the browser mock) everything is a canned stand-in;
// `?update=ready` (or downloading / checking / upToDate / error) previews a
// state.

import { invoke, isTauri } from '@tauri-apps/api/core'
import { emitTo, listen } from '@tauri-apps/api/event'
import { useEffect, useState } from 'react'

export type UpdatePhase = 'checking' | 'upToDate' | 'downloading' | 'ready' | 'error'

/** `boa://update` payload. */
export type UpdateEvent = {
  phase: UpdatePhase
  current: string
  version: string | null
  /** Started by the user (menu, About window), not the periodic check. */
  manual: boolean
  message: string | null
}

export type AppInfo = {
  name: string
  version: string
  arch: string
  /** Empty when unknown. */
  commit: string
  /** RFC 3339; empty when unknown. */
  commitDate: string
  os: string
}

export const UPDATE_EVENT = 'boa://update'
/** About window → main window: install the downloaded update (after the transaction guard). */
export const INSTALL_REQUEST = 'boa://install-update'

const PHASES: UpdatePhase[] = ['checking', 'upToDate', 'downloading', 'ready', 'error']

const mockInfo: AppInfo = {
  name: 'boa-desktop',
  version: '0.0.0-mock',
  arch: 'aarch64',
  commit: '',
  commitDate: new Date().toISOString(),
  os: 'browser'
}

/** The browser preview's state (`?update=<phase>`), if any. */
function mockStatus(): UpdateEvent | null {
  if (typeof location === 'undefined') return null
  const phase = new URLSearchParams(location.search).get('update') as UpdatePhase | null
  if (!phase || !PHASES.includes(phase)) return null
  const hasVersion = phase === 'downloading' || phase === 'ready'
  return {
    phase,
    current: mockInfo.version,
    version: hasVersion ? '9.9.9' : null,
    manual: false,
    message: phase === 'error' ? 'error sending request for url (https://github.com/…)' : null
  }
}

export function appInfo(): Promise<AppInfo> {
  return isTauri() ? invoke<AppInfo>('app_info') : Promise.resolve(mockInfo)
}

export function checkForUpdates(): Promise<UpdateEvent> {
  if (!isTauri()) {
    return Promise.resolve({
      phase: 'upToDate',
      current: mockInfo.version,
      version: null,
      manual: true,
      message: null
    })
  }
  return invoke<UpdateEvent>('check_for_updates')
}

/**
 * Install the downloaded update and relaunch; resolves only on failure.
 * Main window only: go through `restartToUpdate` (GuardDialogs), which asks
 * about open transactions first.
 */
export function installUpdate(): Promise<void> {
  return isTauri() ? invoke<void>('install_update') : Promise.resolve()
}

/** From the About window: ask the main window to restart into the update. */
export function requestInstall(): Promise<void> {
  return isTauri() ? emitTo('main', INSTALL_REQUEST) : Promise.resolve()
}

/** The current update state: the last event, then every new one. */
export function useUpdateStatus(): UpdateEvent | null {
  const [status, setStatus] = useState<UpdateEvent | null>(mockStatus)
  useEffect(() => {
    if (!isTauri()) return
    let cancelled = false
    let unlisten: (() => void) | undefined
    void listen<UpdateEvent>(UPDATE_EVENT, (e) => setStatus(e.payload)).then((u) => {
      if (cancelled) u()
      else unlisten = u
    })
    void invoke<UpdateEvent | null>('update_status').then((s) => {
      if (!cancelled && s) setStatus((cur) => cur ?? s)
    })
    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [])
  return status
}

const ARCH_LABEL: Record<string, string> = { aarch64: 'Apple Silicon', x86_64: 'Intel' }

export function archLabel(arch: string): string {
  return ARCH_LABEL[arch] ?? arch
}

/** "Boa 0.2.0 (Apple Silicon)" plus commit and OS, for bug reports. */
export function versionInfoText(info: AppInfo): string {
  return [
    `Boa ${info.version} (${archLabel(info.arch)})`,
    info.commit && `Commit: ${info.commit}`,
    info.commitDate && `Date: ${info.commitDate}`,
    `OS: ${info.os}`
  ]
    .filter(Boolean)
    .join('\n')
}

/** "Oct 14, 2026 (2 days ago)". */
export function releaseDateLabel(iso: string, now = new Date()): string {
  const d = new Date(iso)
  if (!iso || Number.isNaN(d.getTime())) return ''
  const date = d.toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric' })
  const days = Math.floor((now.getTime() - d.getTime()) / 86_400_000)
  const ago = days <= 0 ? 'today' : days === 1 ? '1 day ago' : `${days} days ago`
  return `${date} (${ago})`
}

// Data source dialog (new / edit a Postgres connection) and the password
// prompt for connections without a stored password. Settings are saved by
// the Rust side in the app config dir, passwords included (plain text for now).
import { useEffect, useState, type FormEvent, type ReactNode } from 'react'
import { CircleAlert, CircleCheck, Trash2 } from 'lucide-react'
import {
  toBackendError,
  backend,
  type ConnConfig,
  type SshAuth,
  type SshTunnel,
  type SslMode
} from '@/lib/backend'
import { configOf, connect, deleteConnection, saveConnection, useStore } from '@/state/store'
import { setPrefs, usePrefs } from '@/state/prefs'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from '@/components/ui/select'
import { Spinner } from '@/components/ui/spinner'
import { cn } from '@/lib/utils'
import { CONNECTION_COLORS, ENVIRONMENTS, ENV_COLOR, isColor } from '@/lib/connTag'
import type { Environment } from '@/state/types'

const SSL_MODES: [SslMode, string][] = [
  ['disable', 'Disable'],
  ['prefer', 'Prefer'],
  ['require', 'Require'],
  ['verify-full', 'Verify full']
]

const blank = (): ConnConfig => ({
  id: crypto.randomUUID(),
  name: '',
  host: 'localhost',
  port: 5432,
  user: 'postgres',
  database: 'postgres',
  sslMode: 'prefer',
  readOnly: false
})

const SSH_AUTH: [SshAuth, string][] = [
  ['agent', 'SSH agent'],
  ['key-file', 'Key file']
]

const blankSsh = (): SshTunnel => ({ enabled: false, host: '', user: '', auth: 'agent' })

/** Tunnel settings to save: trimmed, empty optional fields left out; none if never filled in. */
export function cleanSsh(ssh: SshTunnel | undefined): SshTunnel | undefined {
  if (!ssh) return undefined
  const host = ssh.host.trim()
  const user = ssh.user.trim()
  const identityFile = ssh.identityFile?.trim()
  if (!ssh.enabled && !host && !user && !identityFile) return undefined
  return {
    enabled: ssh.enabled,
    host,
    ...(ssh.port ? { port: ssh.port } : {}),
    user,
    auth: ssh.auth,
    ...(identityFile ? { identityFile } : {})
  }
}

/** An enabled tunnel needs a host, a valid port and, for key-file auth, a key. */
export function sshValid(ssh: SshTunnel | undefined): boolean {
  if (!ssh?.enabled) return true
  if (!ssh.host.trim()) return false
  if (ssh.port != null && (ssh.port < 1 || ssh.port > 65535)) return false
  return ssh.auth !== 'key-file' || !!ssh.identityFile?.trim()
}

/** Fills the fields from `postgres://user:pass@host:port/db?sslmode=…`. */
export function parseUrl(url: string): { config: Partial<ConnConfig>; password?: string } | null {
  let u: URL
  try {
    u = new URL(url.trim())
  } catch {
    return null
  }
  if (u.protocol !== 'postgres:' && u.protocol !== 'postgresql:') return null
  const mode = u.searchParams.get('sslmode')
  const config: Partial<ConnConfig> = {
    host: decodeURIComponent(u.hostname) || 'localhost',
    port: u.port ? Number(u.port) : 5432
  }
  if (u.username) config.user = decodeURIComponent(u.username)
  const db = decodeURIComponent(u.pathname.replace(/^\//, ''))
  if (db) config.database = db
  if (mode === 'disable' || mode === 'prefer' || mode === 'require') config.sslMode = mode
  if (mode === 'verify-full' || mode === 'verify-ca') config.sslMode = 'verify-full'
  return { config, password: u.password ? decodeURIComponent(u.password) : undefined }
}

type Test =
  | { state: 'idle' }
  | { state: 'running' }
  | { state: 'ok'; version: string }
  | { state: 'err'; message: string }

export function ConnectionDialog() {
  const dialog = useStore((s) => s.connectionDialog)
  const editing = useStore((s) => (dialog?.id ? s.connections[dialog.id] : undefined))
  const open = dialog != null
  const [cfg, setCfg] = useState<ConnConfig>(blank)
  const [password, setPassword] = useState('')
  const [passwordTouched, setPasswordTouched] = useState(false)
  const [savePassword, setSavePassword] = useState(true)
  const [url, setUrl] = useState('')
  const [test, setTest] = useState<Test>({ state: 'idle' })
  const [busy, setBusy] = useState(false)
  const [confirmDelete, setConfirmDelete] = useState(false)

  // Reset the form each time the dialog opens.
  // biome-ignore lint/correctness/useExhaustiveDependencies: only on open / target change
  useEffect(() => {
    if (!open) return
    setCfg(editing ? configOf(editing) : blank())
    setPassword('')
    setPasswordTouched(false)
    setSavePassword(editing ? editing.hasPassword : true)
    setUrl('')
    setTest({ state: 'idle' })
    setBusy(false)
    setConfirmDelete(false)
  }, [open, dialog?.id])

  const close = () => useStore.setState({ connectionDialog: null })
  const set = (patch: Partial<ConnConfig>) => {
    setCfg((c) => ({ ...c, ...patch }))
    setTest({ state: 'idle' })
  }
  const named = (): ConnConfig => ({
    ...cfg,
    name: cfg.name.trim() || `${cfg.database}@${cfg.host}`,
    host: cfg.host.trim(),
    user: cfg.user.trim(),
    database: cfg.database.trim() || 'postgres',
    ssh: cleanSsh(cfg.ssh)
  })
  const ssh = cfg.ssh ?? blankSsh()
  const setSsh = (patch: Partial<SshTunnel>) => set({ ssh: { ...ssh, ...patch } })
  /** The password to send: what was typed, or null to use the stored one. */
  const typedPassword = () => (passwordTouched || !editing?.hasPassword ? password : null)

  const runTest = async () => {
    setTest({ state: 'running' })
    try {
      setTest({ state: 'ok', version: await backend.testConnection(named(), typedPassword()) })
    } catch (e) {
      const err = toBackendError(e)
      setTest({
        state: 'err',
        message: err.code === 'password_required' ? 'The server asked for a password' : err.message
      })
    }
  }

  const submit = async (e: FormEvent) => {
    e.preventDefault()
    setBusy(true)
    const config = named()
    const pw = typedPassword()
    try {
      await saveConnection(config, pw, savePassword)
      close()
      if (!editing) {
        // Expand the new connection in the explorer and connect it.
        setPrefs({ treeExpanded: { ...usePrefs.getState().treeExpanded, [config.id]: true } })
        void connect(config.id, savePassword ? undefined : (pw ?? undefined))
      }
    } catch (err) {
      setTest({ state: 'err', message: toBackendError(err).message })
      setBusy(false)
    }
  }

  const remove = async () => {
    if (!editing) return
    if (!confirmDelete) return setConfirmDelete(true)
    try {
      await deleteConnection(editing.id)
      close()
    } catch (err) {
      setTest({ state: 'err', message: toBackendError(err).message })
    }
  }

  const valid =
    cfg.host.trim() && cfg.user.trim() && cfg.port > 0 && cfg.port < 65536 && sshValid(cfg.ssh)

  return (
    <Dialog open={open} onOpenChange={(o) => !o && close()}>
      <DialogContent className="max-h-[calc(100vh-2rem)] overflow-y-auto sm:max-w-[520px]">
        <form onSubmit={submit} className="flex flex-col gap-3">
          <DialogHeader>
            <DialogTitle>
              {editing ? `Edit ${editing.name}` : 'New PostgreSQL data source'}
            </DialogTitle>
            <DialogDescription>
              {backend.real
                ? 'Settings and the password are saved in a config file on this machine.'
                : 'Browser preview: no server is contacted, the sample data is used.'}
            </DialogDescription>
          </DialogHeader>

          {!editing && (
            <Field label="URL">
              <Input
                value={url}
                placeholder="postgresql://user:password@host:5432/database"
                className="font-mono text-xs"
                onChange={(e) => {
                  setUrl(e.target.value)
                  const parsed = parseUrl(e.target.value)
                  if (!parsed) return
                  set(parsed.config)
                  if (parsed.password != null) {
                    setPassword(parsed.password)
                    setPasswordTouched(true)
                  }
                }}
              />
            </Field>
          )}

          <Field label="Name">
            <Input
              autoFocus
              value={cfg.name}
              placeholder={`${cfg.database || 'postgres'}@${cfg.host || 'localhost'}`}
              onChange={(e) => set({ name: e.target.value })}
            />
          </Field>
          <div className="grid grid-cols-[1fr_96px] gap-3">
            <Field label="Host">
              <Input value={cfg.host} onChange={(e) => set({ host: e.target.value })} />
            </Field>
            <Field label="Port">
              <Input
                inputMode="numeric"
                value={String(cfg.port || '')}
                onChange={(e) => set({ port: Number(e.target.value.replace(/\D/g, '')) || 0 })}
              />
            </Field>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <Field label="User">
              <Input
                value={cfg.user}
                autoComplete="off"
                onChange={(e) => set({ user: e.target.value })}
              />
            </Field>
            <Field label="Password">
              <Input
                type="password"
                autoComplete="new-password"
                value={password}
                placeholder={editing?.hasPassword && !passwordTouched ? '<stored>' : ''}
                onChange={(e) => {
                  setPassword(e.target.value)
                  setPasswordTouched(true)
                  setTest({ state: 'idle' })
                }}
              />
            </Field>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <Field label="Database">
              <Input value={cfg.database} onChange={(e) => set({ database: e.target.value })} />
            </Field>
            <Field label="SSL">
              <Select value={cfg.sslMode} onValueChange={(v) => set({ sslMode: v as SslMode })}>
                <SelectTrigger className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {SSL_MODES.map(([v, label]) => (
                    <SelectItem key={v} value={v}>
                      {label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <Field label="Environment">
              <Select
                value={cfg.environment ?? 'none'}
                onValueChange={(v) =>
                  set({ environment: v === 'none' ? undefined : (v as Environment) })
                }
              >
                <SelectTrigger className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="none">None</SelectItem>
                  {ENVIRONMENTS.map(([v, label]) => (
                    <SelectItem key={v} value={v}>
                      {label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>
            <ColorField
              value={cfg.color}
              fallback={cfg.environment ? ENV_COLOR[cfg.environment] : undefined}
              onChange={(color) => set({ color })}
            />
          </div>
          <div className="flex flex-wrap items-center gap-x-5 gap-y-2">
            <Label className="flex w-fit cursor-pointer items-center gap-2 text-sm font-normal text-soft">
              <Checkbox
                checked={savePassword}
                onCheckedChange={(v) => setSavePassword(v === true)}
              />
              Save password
            </Label>
            <Label
              className="flex w-fit cursor-pointer items-center gap-2 text-sm font-normal text-soft"
              title="Sessions run with default_transaction_read_only = on: the server refuses writes"
            >
              <Checkbox
                checked={cfg.readOnly}
                onCheckedChange={(v) => set({ readOnly: v === true })}
              />
              Read-only
            </Label>
          </div>
          {cfg.environment === 'production' && (
            <p className="-mt-1 text-xs text-quiet">
              Destructive statements on production always ask for confirmation.
            </p>
          )}

          <div className="flex flex-col gap-3 border-t border-border pt-3">
            <Label
              className="flex w-fit cursor-pointer items-center gap-2 text-sm font-normal text-soft"
              title="Connect through an SSH server, with the system ssh"
            >
              <Checkbox
                checked={ssh.enabled}
                onCheckedChange={(v) => setSsh({ enabled: v === true })}
              />
              SSH tunnel
            </Label>
            {ssh.enabled && (
              <>
                <div className="grid grid-cols-[1fr_96px] gap-3">
                  <Field label="SSH host">
                    <Input
                      value={ssh.host}
                      placeholder="bastion.example.com or a ~/.ssh/config alias"
                      autoComplete="off"
                      onChange={(e) => setSsh({ host: e.target.value })}
                    />
                  </Field>
                  <Field label="SSH port">
                    <Input
                      inputMode="numeric"
                      value={ssh.port ? String(ssh.port) : ''}
                      placeholder="22"
                      onChange={(e) =>
                        setSsh({ port: Number(e.target.value.replace(/\D/g, '')) || undefined })
                      }
                    />
                  </Field>
                </div>
                <div className="grid grid-cols-2 gap-3">
                  <Field label="SSH user">
                    <Input
                      value={ssh.user}
                      placeholder="from ~/.ssh/config"
                      autoComplete="off"
                      onChange={(e) => setSsh({ user: e.target.value })}
                    />
                  </Field>
                  <Field label="Authentication">
                    <Select value={ssh.auth} onValueChange={(v) => setSsh({ auth: v as SshAuth })}>
                      <SelectTrigger className="w-full">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {SSH_AUTH.map(([v, label]) => (
                          <SelectItem key={v} value={v}>
                            {label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </Field>
                </div>
                {ssh.auth === 'key-file' && (
                  <Field label="Private key file">
                    <Input
                      value={ssh.identityFile ?? ''}
                      placeholder="~/.ssh/id_ed25519"
                      className="font-mono text-xs"
                      onChange={(e) => setSsh({ identityFile: e.target.value })}
                    />
                  </Field>
                )}
                <p className="-mt-1 text-xs text-quiet">
                  Runs the system ssh, so ~/.ssh/config, the agent and known_hosts apply (keys with
                  a passphrase must be in the agent). Host and port above are as the SSH server sees
                  them.
                </p>
              </>
            )}
          </div>

          <TestResult test={test} />

          <DialogFooter className="flex-row items-center sm:justify-between">
            <div className="flex items-center gap-2">
              <Button
                type="button"
                variant="outline"
                size="md"
                disabled={!valid || test.state === 'running'}
                onClick={runTest}
              >
                {test.state === 'running' && <Spinner className="size-3.5" />}
                Test connection
              </Button>
              {editing && (
                <Button
                  type="button"
                  variant={confirmDelete ? 'destructive' : 'ghost'}
                  size="md"
                  onClick={remove}
                  className={cn(!confirmDelete && 'text-quiet')}
                >
                  <Trash2 className="size-3.5" />
                  {confirmDelete ? 'Click to delete' : 'Delete'}
                </Button>
              )}
            </div>
            <div className="flex items-center gap-2">
              <Button type="button" variant="ghost" size="md" onClick={close}>
                Cancel
              </Button>
              <Button type="submit" size="md" disabled={!valid || busy}>
                {editing ? 'Save' : 'Save and connect'}
              </Button>
            </div>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="flex min-w-0 flex-col gap-1.5">
      <span className="text-xs text-quiet">{label}</span>
      {children}
    </label>
  )
}

/** Colour swatches: "auto" (the environment's colour, if any) or a palette colour. */
function ColorField({
  value,
  fallback,
  onChange
}: {
  value: string | undefined
  fallback: string | undefined
  onChange: (color: string | undefined) => void
}) {
  const picked = isColor(value) ? value : undefined
  const swatch =
    'grid size-5 cursor-pointer place-items-center rounded-full outline-none ring-offset-2 ring-offset-background focus-visible:ring-2 focus-visible:ring-ring'
  return (
    <div className="flex min-w-0 flex-col gap-1.5">
      <span className="text-xs text-quiet">Colour</span>
      <div role="radiogroup" aria-label="Colour" className="flex h-9 items-center gap-1.5">
        <button
          type="button"
          role="radio"
          aria-checked={!picked}
          title={fallback ? `Auto (${fallback})` : 'None'}
          onClick={() => onChange(undefined)}
          className={cn(
            swatch,
            'border border-dashed border-dim text-[9px] text-dim',
            !picked && 'ring-2 ring-foreground/60'
          )}
          style={
            fallback
              ? { borderColor: CONNECTION_COLORS[fallback as keyof typeof CONNECTION_COLORS] }
              : undefined
          }
        >
          A
        </button>
        {Object.entries(CONNECTION_COLORS).map(([name, css]) => (
          <button
            key={name}
            type="button"
            role="radio"
            aria-checked={picked === name}
            aria-label={name}
            title={name}
            onClick={() => onChange(name)}
            className={cn(swatch, picked === name && 'ring-2 ring-foreground/60')}
            style={{ background: css }}
          />
        ))}
      </div>
    </div>
  )
}

function TestResult({ test }: { test: Test }) {
  if (test.state === 'ok')
    return (
      <div className="flex items-center gap-2 text-sm text-status-ok">
        <CircleCheck className="size-4 shrink-0" />
        Connected · {test.version}
      </div>
    )
  if (test.state === 'err')
    return (
      <div className="flex items-start gap-2 text-sm break-words text-status-err-text">
        <CircleAlert className="mt-0.5 size-4 shrink-0" />
        <span className="min-w-0 select-text">{test.message}</span>
      </div>
    )
  return null
}

/** Asks for a password when the server wants one and none is stored. */
export function PasswordPrompt() {
  const prompt = useStore((s) => s.passwordPrompt)
  const conn = useStore((s) => (prompt ? s.connections[prompt.connectionId] : undefined))
  const [password, setPassword] = useState('')
  const [save, setSave] = useState(false)
  const [busy, setBusy] = useState(false)
  // biome-ignore lint/correctness/useExhaustiveDependencies: reset per prompt
  useEffect(() => {
    setPassword('')
    setBusy(false)
  }, [prompt?.connectionId, prompt?.error])
  if (!prompt || !conn) return null
  const close = () => useStore.setState({ passwordPrompt: null })
  const submit = async (e: FormEvent) => {
    e.preventDefault()
    setBusy(true)
    if (save) {
      try {
        await saveConnection(configOf(conn), password, true)
      } catch (err) {
        useStore.setState({ passwordPrompt: { ...prompt, error: toBackendError(err).message } })
        setBusy(false)
        return
      }
    }
    await connect(conn.id, password)
    setBusy(false)
  }
  return (
    <Dialog open onOpenChange={(o) => !o && close()}>
      <DialogContent className="sm:max-w-[400px]">
        <form onSubmit={submit} className="flex flex-col gap-3">
          <DialogHeader>
            <DialogTitle>Connect to {conn.name}</DialogTitle>
            <DialogDescription className="font-mono text-xs">
              {conn.user}@{conn.host}:{conn.port}/{conn.database}
            </DialogDescription>
          </DialogHeader>
          <Input
            autoFocus
            type="password"
            placeholder="Password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
          <Label className="flex w-fit cursor-pointer items-center gap-2 text-sm font-normal text-soft">
            <Checkbox checked={save} onCheckedChange={(v) => setSave(v === true)} />
            Save password
          </Label>
          {prompt.error && (
            <div className="flex items-start gap-2 text-sm text-status-err-text">
              <CircleAlert className="mt-0.5 size-4 shrink-0" />
              {prompt.error}
            </div>
          )}
          <DialogFooter>
            <Button type="button" variant="ghost" size="md" onClick={close}>
              Cancel
            </Button>
            <Button type="submit" size="md" disabled={busy}>
              {busy && <Spinner className="size-3.5" />}
              Connect
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

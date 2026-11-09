import { useEffect, useRef, useState } from 'react'
import { webdavHasPassword, webdavSetPassword, webdavTest } from '../../api'
import type { Translate } from '../../i18n'
import type { Settings, WebdavConnection } from '../../persist'
import { ipcErrorText } from './SettingsShared'

export type WebdavSectionProps = {
  settings: Settings
  onChange: (next: Settings) => void
  t: Translate
  visible: boolean
}

/** Form state: the persisted connection plus a password that is never written into settings. */
type Draft = WebdavConnection & { password: string }

type Status = { tone: 'ok' | 'error'; text: string }

/** Plain HTTP puts the password and file bodies on the wire unencrypted. */
const isPlainHttp = (url: string) => /^http:\/\//i.test(url.trim())

const draftFor = (existing: WebdavConnection[]): Draft => {
  let n = 1
  while (existing.some((conn) => conn.id === `conn-${n}`)) n += 1
  return {
    id: `conn-${n}`,
    name: '',
    url: '',
    username: '',
    root: '',
    insecureTls: false,
    password: '',
  }
}

/**
 * Connection list and editor for WebDAV servers. Passwords are written through their own
 * command (Credential Manager); the rest of the connection travels with `settings`, whose
 * save is debounced.
 */
export function WebdavSection({ settings, onChange, t, visible }: WebdavSectionProps) {
  const [draft, setDraft] = useState<Draft | null>(null)
  const [editingExisting, setEditingExisting] = useState(false)
  const [savedPassword, setSavedPassword] = useState(false)
  const [status, setStatus] = useState<Status | null>(null)
  const [testing, setTesting] = useState(false)
  // A probe answers asynchronously; a newer probe (or closing the editor) must win, and
  // the reply must never be shown next to a URL the user has since edited.
  const testSeq = useRef(0)

  // Whether a password exists is backend state, not part of the settings document.
  useEffect(() => {
    if (!draft || !editingExisting) {
      setSavedPassword(false)
      return
    }
    let active = true
    void webdavHasPassword(draft.id)
      .then((has) => {
        if (active) setSavedPassword(has)
      })
      .catch(() => {
        if (active) setSavedPassword(false)
      })
    return () => {
      active = false
    }
    // Only the identity matters; the form object gets a new reference on every keystroke.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draft?.id, editingExisting])

  if (!visible) return null

  const openNew = () => {
    setDraft(draftFor(settings.webdav))
    setEditingExisting(false)
    setStatus(null)
  }

  const openEdit = (conn: WebdavConnection) => {
    setDraft({ ...conn, password: '' })
    setEditingExisting(true)
    setStatus(null)
  }

  const close = () => {
    testSeq.current += 1
    setDraft(null)
    setStatus(null)
    setTesting(false)
  }

  /** Edits one field and drops a probe result that described the previous value. */
  const edit = (patch: Partial<Draft>) => {
    setDraft((prev) => (prev ? { ...prev, ...patch } : prev))
    testSeq.current += 1
    setStatus(null)
    setTesting(false)
  }

  const save = () => {
    if (!draft) return
    const name = draft.name.trim()
    const url = draft.url.trim()
    if (!name || !url) {
      setStatus({ tone: 'error', text: t('settings.webdavInvalid') })
      return
    }
    const connection: WebdavConnection = {
      id: draft.id,
      name,
      url,
      username: draft.username.trim(),
      root: draft.root.trim(),
      insecureTls: draft.insecureTls,
    }
    const finish = () => {
      const webdav = editingExisting
        ? settings.webdav.map((conn) => (conn.id === connection.id ? connection : conn))
        : [...settings.webdav, connection]
      onChange({ ...settings, webdav })
      // A new connection must be in settings before its removal can clean up credentials;
      // the debounced save catches up on its own.
      close()
    }
    if (!draft.password) {
      finish()
      return
    }
    void webdavSetPassword(connection.id, connection.username, draft.password, connection.url)
      .then(finish)
      .catch((err: unknown) =>
        setStatus({ tone: 'error', text: ipcErrorText(err) }),
      )
  }

  const clearPassword = () => {
    if (!draft) return
    void webdavSetPassword(draft.id, draft.username.trim(), '')
      .then(() => {
        setSavedPassword(false)
        setStatus({ tone: 'ok', text: t('settings.webdavPasswordMissing') })
      })
      .catch((err: unknown) => setStatus({ tone: 'error', text: ipcErrorText(err) }))
  }

  const test = () => {
    if (!draft) return
    const connection: WebdavConnection = {
      id: draft.id,
      name: draft.name.trim() || draft.id,
      url: draft.url.trim(),
      username: draft.username.trim(),
      root: draft.root.trim(),
      insecureTls: draft.insecureTls,
    }
    if (!connection.url) {
      setStatus({ tone: 'error', text: t('settings.webdavInvalid') })
      return
    }
    setTesting(true)
    setStatus(null)
    const seq = ++testSeq.current
    void webdavTest(connection, draft.password || undefined)
      .then(() => {
        if (testSeq.current === seq) setStatus({ tone: 'ok', text: t('settings.webdavTestOk') })
      })
      .catch((err: unknown) => {
        if (testSeq.current === seq) setStatus({ tone: 'error', text: ipcErrorText(err) })
      })
      .finally(() => {
        if (testSeq.current === seq) setTesting(false)
      })
  }

  const remove = (conn: WebdavConnection) => {
    if (!window.confirm(t('settings.webdavDeleteConfirm', { name: conn.name }))) return
    onChange({ ...settings, webdav: settings.webdav.filter((entry) => entry.id !== conn.id) })
    // The settings save is debounced; clearing the credential directly also covers a
    // connection removed before its first save ever reached `settings.toml`.
    void webdavSetPassword(conn.id, conn.username, '').catch(() => undefined)
    if (draft?.id === conn.id) close()
  }

  return (
    <section data-webdav>
      <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">
        {t('settings.webdav')}
      </h2>
      <div className="space-y-2 rounded-lg border border-line px-3 py-2">
        <div className="text-[0.65625rem] text-muted-2">{t('settings.webdavHint')}</div>

        {settings.webdav.length ? (
          settings.webdav.map((conn) => (
            <div
              key={conn.id}
              className="flex items-center justify-between gap-2 border-t border-line pt-1.5 text-[0.71875rem]"
            >
              <div className="min-w-0">
                <div className="truncate text-fg">{conn.name}</div>
                <div className="truncate font-mono text-[0.625rem] text-muted-2" title={conn.url}>
                  {conn.url}
                  {conn.root ? `/${conn.root}` : ''}
                </div>
                {isPlainHttp(conn.url) && (
                  <div className="truncate text-[0.625rem] text-warning-strong dark:text-warning-soft">
                    {t('settings.webdavPlainHttpShort')}
                  </div>
                )}
              </div>
              <div className="flex shrink-0 items-center gap-1">
                <button
                  type="button"
                  onClick={() => openEdit(conn)}
                  className="rounded px-1.5 py-0.5 text-[0.6875rem] text-muted hover:bg-surface-3 hover:text-fg"
                >
                  {t('settings.webdavEdit')}
                </button>
                <button
                  type="button"
                  onClick={() => remove(conn)}
                  className="rounded px-1.5 py-0.5 text-[0.6875rem] text-muted hover:bg-surface-3 hover:text-red-500"
                >
                  {t('settings.webdavDelete')}
                </button>
              </div>
            </div>
          ))
        ) : (
          <div className="border-t border-line pt-1.5 text-[0.65625rem] text-muted-2">
            {t('settings.webdavEmpty')}
          </div>
        )}

        {draft ? (
          <div className="space-y-1.5 border-t border-line pt-2">
            <Field label={t('settings.webdavName')}>
              <input
                value={draft.name}
                onChange={(ev) => edit({ name: ev.target.value })}
                placeholder={t('settings.webdavNamePh')}
                className={INPUT}
              />
            </Field>
            <Field label={t('settings.webdavUrl')}>
              <input
                value={draft.url}
                onChange={(ev) => edit({ url: ev.target.value })}
                placeholder={t('settings.webdavUrlPh')}
                spellCheck={false}
                className={INPUT}
              />
            </Field>
            {isPlainHttp(draft.url) && (
              <div className="text-[0.625rem] text-warning-strong dark:text-warning-soft">
                {t('settings.webdavPlainHttp')}
              </div>
            )}
            <div className="grid grid-cols-2 gap-1.5">
              <Field label={t('settings.webdavUsername')}>
                <input
                  value={draft.username}
                  onChange={(ev) => edit({ username: ev.target.value })}
                  placeholder={t('settings.webdavUsernamePh')}
                  spellCheck={false}
                  className={INPUT}
                />
              </Field>
              <Field label={t('settings.webdavRoot')}>
                <input
                  value={draft.root}
                  onChange={(ev) => edit({ root: ev.target.value })}
                  placeholder={t('settings.webdavRootPh')}
                  spellCheck={false}
                  className={INPUT}
                />
              </Field>
            </div>
            <div className="text-[0.625rem] text-muted-2">{t('settings.webdavRootHint')}</div>

            <Field label={t('settings.webdavPassword')}>
              <div className="flex items-center gap-1.5">
                <input
                  type="password"
                  value={draft.password}
                  onChange={(ev) => edit({ password: ev.target.value })}
                  placeholder={
                    editingExisting && savedPassword
                      ? t('settings.webdavPasswordKeep')
                      : t('settings.webdavPasswordPh')
                  }
                  className={INPUT}
                />
                {editingExisting && savedPassword && (
                  <button
                    type="button"
                    onClick={clearPassword}
                    className="h-6 shrink-0 rounded border border-line px-1.5 text-[0.6875rem] text-muted hover:bg-surface-3 hover:text-red-500"
                  >
                    {t('settings.webdavClearPassword')}
                  </button>
                )}
              </div>
            </Field>
            {editingExisting && (
              <div className="text-[0.625rem] text-muted-2">
                {savedPassword
                  ? t('settings.webdavPasswordSaved')
                  : t('settings.webdavPasswordMissing')}
              </div>
            )}

            <label className="flex items-center gap-1.5 text-[0.6875rem] text-fg">
              <input
                type="checkbox"
                checked={draft.insecureTls}
                onChange={(ev) => edit({ insecureTls: ev.target.checked })}
                className="h-3 w-3 accent-[var(--accent-strong)]"
              />
              {t('settings.webdavInsecure')}
            </label>
            <div className="text-[0.625rem] text-muted-2">{t('settings.webdavInsecureHint')}</div>

            {status && (
              <div
                className={`text-[0.65625rem] ${
                  status.tone === 'error' ? 'text-red-500' : 'text-accent'
                }`}
              >
                {status.text}
              </div>
            )}

            <div className="flex items-center justify-end gap-1.5 pt-0.5">
              <button
                type="button"
                onClick={test}
                disabled={testing}
                className="h-6 rounded border border-line px-2 text-[0.6875rem] text-fg hover:bg-surface-3 disabled:opacity-40"
              >
                {testing ? t('settings.webdavTesting') : t('settings.webdavTest')}
              </button>
              <button
                type="button"
                onClick={save}
                className="h-6 rounded bg-accent-strong px-2 text-[0.6875rem] text-white hover:bg-accent"
              >
                {t('settings.webdavSave')}
              </button>
              <button
                type="button"
                onClick={close}
                className="h-6 rounded border border-line px-2 text-[0.6875rem] text-fg hover:bg-surface-3"
              >
                {t('settings.webdavCancel')}
              </button>
            </div>
          </div>
        ) : (
          <button
            type="button"
            onClick={openNew}
            className="h-6 rounded border border-line px-2 text-[0.6875rem] text-fg hover:bg-surface-3"
          >
            {t('settings.webdavAdd')}
          </button>
        )}
      </div>
    </section>
  )
}

const INPUT =
  'h-6 w-full min-w-0 rounded border border-line bg-surface-2 px-1.5 text-[0.6875rem] outline-none placeholder:text-muted-2 focus:border-accent-soft focus:bg-surface'

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="block">
      <span className="mb-0.5 block text-[0.65625rem] text-muted-2">{label}</span>
      {children}
    </label>
  )
}

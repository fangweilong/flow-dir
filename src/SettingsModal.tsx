import { useEffect, useMemo, useState } from 'react'
import { appInfo, configPath, everythingStatus, explorerOpen, pickFolder, restartApp, updateCheck, updateInstall } from './api'
import { makeT } from './i18n'
import type { Key } from './i18n'
import { DEFAULT_BINDINGS, SHORTCUTS, bindingFromEvent, formatBinding, resolveShortcuts, sameBinding } from './shortcuts'
import type { ShortcutId } from './shortcuts'
import type { AppInfo, EverythingStatus, Lang, Theme, UpdateStatus } from './types'
import type { Settings } from './persist'

type Props = {
  settings: Settings
  lang: Lang
  onChange: (next: Settings) => void
  onClose: () => void
}

type SectionId = 'general' | 'shortcuts' | 'graphics' | 'quickAccess' | 'preview' | 'config' | 'updates' | 'about'

const SECTIONS: { id: SectionId; label: Key }[] = [
  { id: 'general', label: 'settings.general' },
  { id: 'shortcuts', label: 'settings.shortcuts' },
  { id: 'graphics', label: 'settings.graphics' },
  { id: 'quickAccess', label: 'settings.quickAccess' },
  { id: 'preview', label: 'settings.previewSection' },
  { id: 'config', label: 'settings.configDir' },
  { id: 'updates', label: 'settings.updates' },
  { id: 'about', label: 'settings.about' },
]

const THEMES: Theme[] = ['system', 'light', 'dark']
const UPDATE_INTERVALS = [0, 6, 12, 24, 72]

/** IPC failures arrive as the serialized ApiError, so `String(err)` would say `[object Object]`. */
const ipcErrorText = (err: unknown) => {
  if (err && typeof err === 'object' && 'code' in err) {
    const { code, detail } = err as { code: string; detail?: string }
    return detail ? `${code}: ${detail}` : code
  }
  return String(err)
}

function ToggleRow({
  label,
  hint,
  value,
  onChange,
}: {
  label: string
  hint?: string
  value: boolean
  onChange: (value: boolean) => void
}) {
  return (
    <div className="flex items-center justify-between rounded-lg border border-line px-3 py-2">
      <div className="min-w-0">
        <div className="text-[12px] text-fg">{label}</div>
        {hint && <div className="truncate text-[10.5px] text-muted-2">{hint}</div>}
      </div>
      <button
        type="button"
        role="switch"
        aria-checked={value}
        data-toggle={label}
        onClick={() => onChange(!value)}
        className={`relative h-5 w-9 shrink-0 rounded-full transition-colors ${value ? 'bg-blue-600' : 'bg-surface-3'}`}
      >
        <span
          className={`absolute top-0.5 h-4 w-4 rounded-full bg-white shadow transition-[left] ${value ? 'left-[18px]' : 'left-0.5'}`}
        />
      </button>
    </div>
  )
}

export function SettingsModal({ settings, lang, onChange, onClose }: Props) {
  const t = makeT(lang)
  const [info, setInfo] = useState<AppInfo | null>(null)
  const [everything, setEverything] = useState<EverythingStatus | null>(null)
  const [configDir, setConfigDir] = useState('')
  const [placeName, setPlaceName] = useState('')
  const [placePath, setPlacePath] = useState('')
  const [externalExt, setExternalExt] = useState('')
  const [externalCommand, setExternalCommand] = useState('')
  const [section, setSection] = useState<SectionId>('general')
  const [query, setQuery] = useState('')
  const [capturing, setCapturing] = useState<ShortcutId | null>(null)
  const [conflict, setConflict] = useState<string | null>(null)
  const [update, setUpdate] = useState<UpdateStatus | null>(null)
  const [checking, setChecking] = useState(false)
  const [installing, setInstalling] = useState(false)
  const [updateError, setUpdateError] = useState<string | null>(null)

  const keys = useMemo(() => resolveShortcuts(settings.shortcuts), [settings.shortcuts])
  const needle = query.trim().toLowerCase()

  // Section-level search index: every visible label is covered, so "gpu" or "docx" lands
  // on the right page without the user knowing the section names.
  const searchIndex: Record<SectionId, string> = {
    general: [
      t('settings.general'),
      t('settings.appearance'),
      t('settings.theme'),
      t('settings.themeSystem'),
      t('settings.themeLight'),
      t('settings.themeDark'),
      t('settings.language'),
      t('settings.behavior'),
      t('settings.previewOpen'),
      t('sidebar.toggle'),
      t('settings.showHidden'),
      t('settings.useEverything'),
    ].join(' '),
    shortcuts: [t('settings.shortcuts'), ...SHORTCUTS.map((shortcut) => t(shortcut.label))].join(' '),
    graphics: [t('settings.graphics'), t('settings.gpuAccel'), t('settings.restartNow')].join(' '),
    quickAccess: [t('settings.quickAccess'), ...settings.customPlaces.map((place) => place.name)].join(' '),
    preview: [t('settings.previewSection'), t('settings.externalPreview'), ...settings.externalPreview.map((rule) => rule.ext)].join(' '),
    config: [t('settings.configDir'), t('settings.configOpen')].join(' '),
    updates: [t('settings.updates'), t('settings.updateInterval'), t('settings.updateCheck')].join(' '),
    about: [t('settings.about'), t('settings.version'), t('settings.dependencies')].join(' '),
  }
  const sectionClass = (id: SectionId) => {
    if (needle) return searchIndex[id].toLowerCase().includes(needle) ? 'block' : 'hidden'
    return section === id ? 'block' : 'hidden'
  }
  const noMatch = needle !== '' && !SECTIONS.some((entry) => searchIndex[entry.id].toLowerCase().includes(needle))

  useEffect(() => {
    void appInfo().then(setInfo).catch(() => undefined)
    void everythingStatus().then(setEverything).catch(() => undefined)
    void configPath().then(setConfigDir).catch(() => undefined)
    // Interval-guarded: usually returns the cached release without touching the network.
    void updateCheck(false).then(setUpdate).catch(() => undefined)
  }, [])

  const runUpdateCheck = () => {
    setChecking(true)
    setUpdateError(null)
    void updateCheck(true)
      .then(setUpdate)
      .catch((err: unknown) => setUpdateError(ipcErrorText(err)))
      .finally(() => setChecking(false))
  }

  const runUpdateInstall = () => {
    setInstalling(true)
    setUpdateError(null)
    // A successful install exits the app, so only failures need to clear the flag.
    void updateInstall().catch((err: unknown) => {
      setInstalling(false)
      setUpdateError(ipcErrorText(err))
    })
  }

  const builtinPlaces: { key: string; label: string }[] = [
    { key: 'home', label: t('place.home') },
    { key: 'desktop', label: t('place.desktop') },
    { key: 'downloads', label: t('place.downloads') },
    { key: 'documents', label: t('place.documents') },
  ]
  const visibleBuiltins = builtinPlaces.filter((place) => !settings.hiddenPlaces.includes(place.key))

  /** Adds the typed entry; a path is required, the name falls back to its last segment. */
  const addPlace = () => {
    const path = placePath.trim()
    if (!path) return
    const name = placeName.trim() || path.split(/[\\/]/).filter(Boolean).pop() || path
    const exists = settings.customPlaces.some((place) => place.path.toLowerCase() === path.toLowerCase())
    if (!exists) {
      onChange({ ...settings, customPlaces: [...settings.customPlaces, { name, path }] })
    }
    setPlaceName('')
    setPlacePath('')
  }

  /** Extensions are compared lower-case and without a leading dot. */
  const addExternal = () => {
    const ext = externalExt.trim().replace(/^\.+/, '').toLowerCase()
    const command = externalCommand.trim()
    if (!ext || !command) return
    const rest = settings.externalPreview.filter((rule) => rule.ext !== ext)
    onChange({ ...settings, externalPreview: [...rest, { ext, command }] })
    setExternalExt('')
    setExternalCommand('')
  }

  useEffect(() => {
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  // Capture runs in the capture phase so the window shortcuts and the modal's own Escape
  // handler never see the keys being recorded.
  useEffect(() => {
    if (!capturing) return
    const onKey = (ev: KeyboardEvent) => {
      ev.preventDefault()
      ev.stopPropagation()
      if (ev.key === 'Escape') {
        setCapturing(null)
        setConflict(null)
        return
      }
      const binding = bindingFromEvent(ev)
      if (!binding) return
      const clash = SHORTCUTS.find((entry) => entry.id !== capturing && sameBinding(keys[entry.id], binding))
      if (clash) {
        setConflict(t('settings.shortcutConflict', { name: t(clash.label) }))
        return
      }
      const rest = settings.shortcuts.filter((entry) => entry.id !== capturing)
      onChange({
        ...settings,
        shortcuts: binding === DEFAULT_BINDINGS[capturing] ? rest : [...rest, { id: capturing, keys: binding }],
      })
      setCapturing(null)
      setConflict(null)
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [capturing, keys, settings, onChange, t])

  const themeLabel = (theme: Theme) =>
    theme === 'system' ? t('settings.themeSystem') : theme === 'light' ? t('settings.themeLight') : t('settings.themeDark')

  const updateMessage = checking
    ? t('settings.updateChecking')
    : update?.error
      ? t('settings.updateFailed', { code: update.error.code })
      : update?.available
        ? t('settings.updateAvailable', { version: update.latest ?? '' })
        : update?.latest
          ? t('settings.updateLatest')
          : t('settings.updateIdle')
  const updateDetail = update?.error ? (update.error.detail ?? update.error.code) : undefined
  const checkedAt = update?.lastCheck
    ? new Date(update.lastCheck * 1000).toLocaleString(lang === 'zh-CN' ? 'zh-CN' : 'en-US', {
        dateStyle: 'short',
        timeStyle: 'short',
      })
    : null

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/40 p-6 backdrop-blur-[2px]"
      onMouseDown={(ev) => {
        if (ev.target === ev.currentTarget) onClose()
      }}
    >
      <div className="flex h-[600px] max-h-[85vh] w-[760px] flex-col overflow-hidden rounded-xl border border-line bg-surface shadow-2xl">
        <header className="flex items-center gap-3 border-b border-line px-4 py-3">
          <h1 className="text-[13px] font-semibold text-fg-strong">{t('settings.title')}</h1>
          <input
            value={query}
            onChange={(ev) => setQuery(ev.target.value)}
            placeholder={t('settings.searchPh')}
            data-settings-search
            className="ml-auto h-6 w-[200px] rounded border border-line bg-surface-2 px-2 text-[11px] outline-none placeholder:text-muted-2 focus:border-blue-400 focus:bg-surface"
          />
          <button
            type="button"
            onClick={onClose}
            className="grid h-6 w-6 place-items-center rounded text-muted hover:bg-surface-3 hover:text-fg"
            title={t('settings.close')}
          >
            ×
          </button>
        </header>

        <div className="flex min-h-0 flex-1">
          {!needle && (
            <nav className="w-[136px] shrink-0 space-y-0.5 overflow-y-auto border-r border-line bg-surface-2 p-1.5" data-settings-nav>
              {SECTIONS.map((entry) => (
                <button
                  key={entry.id}
                  type="button"
                  onClick={() => setSection(entry.id)}
                  className={`w-full rounded px-2 py-1.5 text-left text-[11.5px] ${
                    section === entry.id
                      ? 'bg-surface font-medium text-fg-strong shadow-sm'
                      : 'text-muted hover:bg-surface-3 hover:text-fg'
                  }`}
                >
                  {t(entry.label)}
                </button>
              ))}
            </nav>
          )}

          <div className="min-w-0 flex-1 space-y-5 overflow-y-auto px-4 py-4">
            {noMatch && <div className="text-[11.5px] text-muted-2">{t('settings.noMatch')}</div>}
          <section className={sectionClass('general')}>
            <h2 className="pb-2 text-[11px] font-semibold tracking-wide text-muted-2">{t('settings.appearance')}</h2>
            <div className="flex items-center justify-between rounded-lg border border-line px-3 py-2">
              <span className="text-[12px] text-fg">{t('settings.theme')}</span>
              <div className="flex gap-0.5 rounded-md bg-surface-3 p-0.5">
                {THEMES.map((theme) => (
                  <button
                    key={theme}
                    type="button"
                    onClick={() => onChange({ ...settings, theme })}
                    className={`rounded px-2.5 py-1 text-[11.5px] ${
                      settings.theme === theme
                        ? 'bg-surface font-medium text-fg-strong shadow-sm'
                        : 'text-muted hover:text-fg'
                    }`}
                  >
                    {themeLabel(theme)}
                  </button>
                ))}
              </div>
            </div>
          </section>

          <section className={sectionClass('general')}>
            <h2 className="pb-2 text-[11px] font-semibold tracking-wide text-muted-2">{t('settings.language')}</h2>
            <div className="flex items-center justify-between rounded-lg border border-line px-3 py-2">
              <span className="text-[12px] text-fg">Language / 语言</span>
              <div className="flex gap-0.5 rounded-md bg-surface-3 p-0.5">
                {(['zh-CN', 'en'] as Lang[]).map((code) => (
                  <button
                    key={code}
                    type="button"
                    onClick={() => onChange({ ...settings, language: code })}
                    className={`rounded px-2.5 py-1 text-[11.5px] ${
                      settings.language === code
                        ? 'bg-surface font-medium text-fg-strong shadow-sm'
                        : 'text-muted hover:text-fg'
                    }`}
                  >
                    {code === 'zh-CN' ? '中文' : 'English'}
                  </button>
                ))}
              </div>
            </div>
          </section>

          <section className={sectionClass('general')}>
            <h2 className="pb-2 text-[11px] font-semibold tracking-wide text-muted-2">{t('settings.behavior')}</h2>
            <div className="space-y-2">
              <ToggleRow
                label={t('settings.previewOpen')}
                value={settings.previewOpen}
                onChange={(value) => onChange({ ...settings, previewOpen: value })}
              />
              <ToggleRow
                label={t('sidebar.toggle')}
                value={settings.sidebarOpen}
                onChange={(value) => onChange({ ...settings, sidebarOpen: value })}
              />
              <ToggleRow
                label={t('settings.showHidden')}
                value={settings.showHidden}
                onChange={(value) => onChange({ ...settings, showHidden: value })}
              />
              <ToggleRow
                label={t('settings.useEverything')}
                hint={
                  everything
                    ? everything.usable
                      ? `Everything ${everything.version ?? ''}`.trim()
                      : t('settings.everythingOff')
                    : '…'
                }
                value={settings.useEverything}
                onChange={(value) => onChange({ ...settings, useEverything: value })}
              />
            </div>
          </section>

          <section className={sectionClass('shortcuts')}>
            <h2 className="pb-2 text-[11px] font-semibold tracking-wide text-muted-2">{t('settings.shortcuts')}</h2>
            <div className="space-y-2" data-shortcuts>
              <div className="text-[10.5px] text-muted-2">{t('settings.shortcutsHint')}</div>
              {(['app', 'pane'] as const).map((scope) => (
                <div key={scope} className="overflow-hidden rounded-lg border border-line">
                  <div className="border-b border-line bg-surface-2 px-3 py-1 text-[10px] font-semibold uppercase tracking-wider text-muted-2">
                    {scope === 'app' ? t('settings.shortcutApp') : t('settings.shortcutPane')}
                  </div>
                  {SHORTCUTS.filter((shortcut) => shortcut.scope === scope).map((shortcut) => {
                    const overridden = !sameBinding(keys[shortcut.id], DEFAULT_BINDINGS[shortcut.id])
                    return (
                      <div
                        key={shortcut.id}
                        className="flex items-center justify-between gap-2 border-b border-line px-3 py-1.5 text-[11.5px] last:border-b-0"
                      >
                        <span className="min-w-0 truncate text-fg">{t(shortcut.label)}</span>
                        <span className="flex shrink-0 items-center gap-1">
                          <button
                            type="button"
                            onClick={() => {
                              setCapturing(shortcut.id)
                              setConflict(null)
                            }}
                            data-shortcut={shortcut.id}
                            className={`min-w-[92px] rounded border px-2 py-0.5 font-mono text-[11px] ${
                              capturing === shortcut.id
                                ? 'border-blue-400 bg-blue-500/10 text-blue-500'
                                : 'border-line bg-surface-2 text-fg hover:bg-surface-3'
                            }`}
                          >
                            {capturing === shortcut.id
                              ? t('settings.shortcutCapturing')
                              : formatBinding(keys[shortcut.id])}
                          </button>
                          {overridden && (
                            <button
                              type="button"
                              onClick={() =>
                                onChange({
                                  ...settings,
                                  shortcuts: settings.shortcuts.filter((entry) => entry.id !== shortcut.id),
                                })
                              }
                              className="rounded px-1 py-0.5 text-[10.5px] text-muted-2 hover:bg-surface-3 hover:text-fg"
                            >
                              {t('settings.shortcutReset')}
                            </button>
                          )}
                        </span>
                      </div>
                    )
                  })}
                </div>
              ))}
              <div className="flex items-center justify-between gap-2 pt-0.5">
                <span className="min-h-[14px] text-[10.5px] text-red-500">{conflict ?? ''}</span>
                <button
                  type="button"
                  onClick={() => onChange({ ...settings, shortcuts: [] })}
                  disabled={!settings.shortcuts.length}
                  className="shrink-0 rounded border border-line px-2 py-0.5 text-[11px] text-fg hover:bg-surface-3 disabled:opacity-40"
                >
                  {t('settings.shortcutResetAll')}
                </button>
              </div>
            </div>
          </section>

          <section className={sectionClass('graphics')}>
            <h2 className="pb-2 text-[11px] font-semibold tracking-wide text-muted-2">{t('settings.graphics')}</h2>
            <div data-gpu-section>
              <ToggleRow
                label={t('settings.gpuAccel')}
                hint={t('settings.gpuAccelHint')}
                value={settings.gpuAccel}
                onChange={(value) => onChange({ ...settings, gpuAccel: value })}
              />
              <div className="mt-1.5 flex items-center justify-between gap-2 rounded-lg border border-line px-3 py-2">
                <span className="text-[10.5px] text-muted-2">{t('settings.gpuRestartHint')}</span>
                <button
                  type="button"
                  onClick={() => void restartApp().catch(() => undefined)}
                  className="shrink-0 rounded-md border border-line px-2 py-1 text-[11px] text-fg hover:bg-surface-3"
                >
                  {t('settings.restartNow')}
                </button>
              </div>
            </div>
          </section>

          <section className={sectionClass('quickAccess')}>
            <h2 className="pb-2 text-[11px] font-semibold tracking-wide text-muted-2">{t('settings.quickAccess')}</h2>
            <div className="space-y-1.5 rounded-lg border border-line px-3 py-2" data-quick-access>
              <div className="pb-1 text-[10.5px] text-muted-2">{t('settings.quickAccessHint')}</div>
              {visibleBuiltins.map((place) => (
                <div key={place.key} className="flex items-center justify-between gap-2 text-[11.5px]">
                  <span className="truncate text-fg">{place.label}</span>
                  <div className="flex shrink-0 items-center gap-2">
                    <span className="text-[10px] uppercase tracking-wide text-muted-2">
                      {t('settings.placeBuiltin')}
                    </span>
                    <button
                      type="button"
                      onClick={() =>
                        onChange({ ...settings, hiddenPlaces: [...settings.hiddenPlaces, place.key] })
                      }
                      className="rounded px-1.5 py-0.5 text-[11px] text-muted hover:bg-surface-3 hover:text-red-500"
                    >
                      {t('settings.placeDelete')}
                    </button>
                  </div>
                </div>
              ))}
              {settings.customPlaces.map((place) => (
                <div key={place.path} className="flex items-center justify-between gap-2 text-[11.5px]">
                  <span className="min-w-0">
                    <span className="text-fg">{place.name}</span>
                    <span className="ml-2 truncate text-[10.5px] text-muted-2" title={place.path}>
                      {place.path}
                    </span>
                  </span>
                  <button
                    type="button"
                    onClick={() =>
                      onChange({
                        ...settings,
                        customPlaces: settings.customPlaces.filter((entry) => entry.path !== place.path),
                      })
                    }
                    className="shrink-0 rounded px-1.5 py-0.5 text-[11px] text-muted hover:bg-surface-3 hover:text-red-500"
                  >
                    {t('settings.placeDelete')}
                  </button>
                </div>
              ))}
              {settings.hiddenPlaces.length > 0 && (
                <button
                  type="button"
                  onClick={() => onChange({ ...settings, hiddenPlaces: [] })}
                  className="rounded px-1.5 py-0.5 text-[11px] text-blue-500 hover:bg-surface-3"
                >
                  {t('settings.placeRestore')}
                </button>
              )}
              <div className="flex items-center gap-1.5 border-t border-line pt-2">
                <input
                  value={placeName}
                  onChange={(ev) => setPlaceName(ev.target.value)}
                  placeholder={t('settings.placeNamePh')}
                  data-place-name
                  className="h-6 w-[96px] rounded border border-line bg-surface-2 px-1.5 text-[11px] outline-none placeholder:text-muted-2 focus:border-blue-400 focus:bg-surface"
                />
                <input
                  value={placePath}
                  onChange={(ev) => setPlacePath(ev.target.value)}
                  onKeyDown={(ev) => {
                    if (ev.key === 'Enter') addPlace()
                  }}
                  placeholder={t('settings.placePathPh')}
                  data-place-path
                  className="h-6 min-w-0 flex-1 rounded border border-line bg-surface-2 px-1.5 text-[11px] outline-none placeholder:text-muted-2 focus:border-blue-400 focus:bg-surface"
                />
                <button
                  type="button"
                  onClick={() => {
                    void pickFolder(t('settings.placePath'), placePath || configDir)
                      .then((picked) => {
                        if (picked) setPlacePath(picked)
                      })
                      .catch(() => undefined)
                  }}
                  className="h-6 shrink-0 rounded border border-line px-1.5 text-[11px] text-fg hover:bg-surface-3"
                >
                  {t('settings.placeBrowse')}
                </button>
                <button
                  type="button"
                  onClick={addPlace}
                  disabled={!placePath.trim()}
                  className="h-6 shrink-0 rounded bg-blue-600 px-2 text-[11px] text-white hover:bg-blue-500 disabled:opacity-40"
                >
                  {t('settings.placeAdd')}
                </button>
              </div>
            </div>
          </section>

          <section className={sectionClass('preview')}>
            <h2 className="pb-2 text-[11px] font-semibold tracking-wide text-muted-2">
              {t('settings.externalPreview')}
            </h2>
            <div className="space-y-1.5 rounded-lg border border-line px-3 py-2" data-external-preview>
              <div className="pb-1 text-[10.5px] text-muted-2">{t('settings.externalPreviewHint')}</div>
              <div className="pb-1 text-[10.5px] text-muted-2">
                {t('settings.externalKey', { keys: formatBinding(keys.externalPreview) })}
              </div>
              {settings.externalPreview.map((rule) => (
                <div key={rule.ext} className="flex items-center justify-between gap-2 text-[11.5px]">
                  <span className="min-w-0">
                    <span className="font-mono text-fg">.{rule.ext}</span>
                    <span className="ml-2 truncate font-mono text-[10.5px] text-muted-2" title={rule.command}>
                      {rule.command}
                    </span>
                  </span>
                  <button
                    type="button"
                    onClick={() =>
                      onChange({
                        ...settings,
                        externalPreview: settings.externalPreview.filter((entry) => entry.ext !== rule.ext),
                      })
                    }
                    className="shrink-0 rounded px-1.5 py-0.5 text-[11px] text-muted hover:bg-surface-3 hover:text-red-500"
                  >
                    {t('settings.placeDelete')}
                  </button>
                </div>
              ))}
              <div className="flex items-center gap-1.5 border-t border-line pt-2">
                <input
                  value={externalExt}
                  onChange={(ev) => setExternalExt(ev.target.value)}
                  placeholder={t('settings.externalExtPh')}
                  data-external-ext
                  className="h-6 w-[72px] shrink-0 rounded border border-line bg-surface-2 px-1.5 text-[11px] outline-none placeholder:text-muted-2 focus:border-blue-400 focus:bg-surface"
                />
                <input
                  value={externalCommand}
                  onChange={(ev) => setExternalCommand(ev.target.value)}
                  onKeyDown={(ev) => {
                    if (ev.key === 'Enter') addExternal()
                  }}
                  placeholder={t('settings.externalCommandPh')}
                  title={t('settings.externalCommand')}
                  data-external-command
                  className="h-6 min-w-0 flex-1 rounded border border-line bg-surface-2 px-1.5 text-[11px] outline-none placeholder:text-muted-2 focus:border-blue-400 focus:bg-surface"
                />
                <button
                  type="button"
                  onClick={addExternal}
                  disabled={!externalExt.trim() || !externalCommand.trim()}
                  className="h-6 shrink-0 rounded bg-blue-600 px-2 text-[11px] text-white hover:bg-blue-500 disabled:opacity-40"
                >
                  {t('settings.placeAdd')}
                </button>
              </div>
            </div>
          </section>

          <section className={sectionClass('config')}>
            <h2 className="pb-2 text-[11px] font-semibold tracking-wide text-muted-2">{t('settings.configDir')}</h2>
            <div className="flex items-center gap-2 rounded-lg border border-line px-3 py-2">
              <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-muted" title={configDir} data-config-dir>
                {configDir || '…'}
              </span>
              <button
                type="button"
                disabled={!configDir}
                onClick={() => void explorerOpen(configDir).catch(() => undefined)}
                className="shrink-0 rounded-md border border-line px-2 py-1 text-[11px] text-fg hover:bg-surface-3 disabled:opacity-40"
              >
                {t('settings.configOpen')}
              </button>
            </div>
          </section>

          <section className={sectionClass('updates')}>
            <h2 className="pb-2 text-[11px] font-semibold tracking-wide text-muted-2">{t('settings.updates')}</h2>
            <div className="space-y-2 rounded-lg border border-line px-3 py-2" data-update-section>
              <div className="flex items-center justify-between gap-2">
                <div className="min-w-0">
                  <div className="text-[12px] text-fg">
                    v{info?.version ?? update?.current ?? '…'}
                    <span className="ml-2 text-[10.5px] text-muted-2">
                      {update?.flavor === 'installer'
                        ? t('settings.updateFlavorInstaller')
                        : t('settings.updateFlavorPortable')}
                    </span>
                  </div>
                  <div className="truncate text-[10.5px] text-muted-2" title={updateDetail} data-update-status>
                    {updateMessage}
                    {checkedAt && <span className="ml-1 text-muted-2">· {t('settings.updateLastCheck', { time: checkedAt })}</span>}
                  </div>
                </div>
                <button
                  type="button"
                  onClick={runUpdateCheck}
                  disabled={checking || installing}
                  data-update-check
                  className="shrink-0 rounded-md border border-line px-2 py-1 text-[11px] text-fg hover:bg-surface-3 disabled:opacity-40"
                >
                  {t('settings.updateCheck')}
                </button>
              </div>
              {update?.available && (
                <button
                  type="button"
                  onClick={runUpdateInstall}
                  disabled={installing}
                  data-update-install
                  className="w-full rounded-md bg-blue-600 px-2 py-1 text-[11.5px] text-white hover:bg-blue-500 disabled:opacity-50"
                >
                  {installing ? t('settings.updateInstalling') : t('settings.updateInstall')}
                </button>
              )}
              {updateError && <div className="text-[10.5px] text-red-500">{updateError}</div>}
              <div className="flex items-center justify-between gap-2 border-t border-line pt-2">
                <span className="text-[12px] text-fg">{t('settings.updateInterval')}</span>
                <select
                  value={settings.updateIntervalHours}
                  onChange={(ev) => onChange({ ...settings, updateIntervalHours: Number(ev.target.value) })}
                  data-update-interval
                  className="h-6 rounded border border-line bg-surface-2 px-1 text-[11px] text-fg outline-none focus:border-blue-400"
                >
                  {UPDATE_INTERVALS.map((hours) => (
                    <option key={hours} value={hours}>
                      {hours === 0
                        ? t('settings.updateManualOnly')
                        : t('settings.updateEvery', { hours })}
                    </option>
                  ))}
                </select>
              </div>
            </div>
          </section>

          <section className={sectionClass('about')}>
            <h2 className="pb-2 text-[11px] font-semibold tracking-wide text-muted-2">{t('settings.about')}</h2>
            <div className="rounded-lg border border-line px-3 py-2 text-[11.5px]">
              <div className="flex justify-between py-0.5">
                <span className="text-muted">{t('settings.version')}</span>
                <span className="font-mono text-fg">{info?.version ?? '…'}</span>
              </div>
              <div className="mt-2 border-t border-line pt-2">
                <div className="pb-1 text-[10.5px] text-muted-2">{t('settings.dependencies')}</div>
                {(['rust', 'npm'] as const).map((kind) => {
                  const rows = (info?.dependencies ?? []).filter((dep) => dep.kind === kind)
                  if (!rows.length) return null
                  return (
                    <div key={kind} className="mb-2 last:mb-0">
                      <div className="pb-0.5 text-[9.5px] font-semibold uppercase tracking-wider text-muted-2">
                        {kind}
                      </div>
                      <div className="grid grid-cols-[minmax(0,1fr)_72px_128px] items-baseline gap-x-3 gap-y-1">
                        {rows.map((dep) => (
                          <div key={dep.name} className="contents">
                            <span className="truncate font-mono text-[11px] text-fg" title={dep.name}>
                              {dep.name}
                            </span>
                            <span className="text-right font-mono text-[11px] tabular text-muted">
                              {dep.version}
                            </span>
                            <span className="truncate text-right text-[10.5px] text-muted-2" title={dep.license}>
                              {dep.license}
                            </span>
                          </div>
                        ))}
                      </div>
                    </div>
                  )
                })}
              </div>
            </div>
          </section>
          </div>
        </div>

        <footer className="flex justify-end border-t border-line px-4 py-2.5">
          <button
            type="button"
            onClick={onClose}
            className="rounded-md border border-line px-3 py-1 text-[11.5px] text-fg hover:bg-surface-3"
          >
            {t('settings.close')}
          </button>
        </footer>
      </div>
    </div>
  )
}

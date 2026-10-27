import { useEffect, useMemo, useRef, useState } from 'react'
import { appInfo, configPath, diagnosticsExport, everythingStatus, logDirPath, pickFolder, restartApp, settingsSave, updateCheck, updateInstall } from './api'
import { makeT } from './i18n'
import type { Key } from './i18n'
import { DEFAULT_BINDINGS, SHORTCUTS, bindingFromEvent, formatBinding, resolveShortcuts, sameBinding } from './shortcuts'
import type { ShortcutId } from './shortcuts'
import type { AppInfo, EverythingStatus, Lang, Theme, UpdateStatus } from './types'
import { THEMES, themeIsDark } from './themes'
import type { LogLevel, Settings } from './persist'
import {
  DEFAULT_FONT_FAMILY,
  DEFAULT_LIST_FONT_SIZE,
  DEFAULT_NAV_FONT_SIZE,
  DEFAULT_TAB_FONT_SIZE,
  FONT_FAMILIES,
  FONT_SIZE_MAX,
  FONT_SIZE_MIN,
  REGION_FONT_SIZE_MIN,
  clampFontSize,
  clampRegionFontSize,
} from './fonts'

type Props = {
  settings: Settings
  lang: Lang
  onChange: (next: Settings) => void
  /** Opens a folder inside FlowDir itself; the settings modal closes first. */
  onOpenLocation: (path: string) => void
  onClose: () => void
}

type SectionId = 'general' | 'shortcuts' | 'graphics' | 'quickAccess' | 'preview' | 'config' | 'updates' | 'diagnostics' | 'about'

const SECTIONS: { id: SectionId; label: Key }[] = [
  { id: 'general', label: 'settings.general' },
  { id: 'shortcuts', label: 'settings.shortcuts' },
  { id: 'graphics', label: 'settings.graphics' },
  { id: 'quickAccess', label: 'settings.quickAccess' },
  { id: 'preview', label: 'settings.previewSection' },
  { id: 'config', label: 'settings.configDir' },
  { id: 'updates', label: 'settings.updates' },
  { id: 'diagnostics', label: 'settings.diagnostics' },
  { id: 'about', label: 'settings.about' },
]

const UPDATE_INTERVALS = [0, 6, 12, 24, 72]
const LOG_LEVELS: LogLevel[] = ['error', 'warn', 'info', 'debug']

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
        <div className="text-[0.75rem] text-fg">{label}</div>
        {hint && <div className="truncate text-[0.65625rem] text-muted-2">{hint}</div>}
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
          className={`absolute top-0.5 h-4 w-4 rounded-full bg-white shadow transition-[left] ${value ? 'left-[1.125rem]' : 'left-0.5'}`}
        />
      </button>
    </div>
  )
}

/**
 * Miniature file list for the theme picker. The real tokens paint it, so the swatch
 * cannot drift from the palette; `system` splits into the two base schemes.
 */
function ThemeSwatch({ theme }: { theme: Theme }) {
  if (theme === 'system') {
    return (
      <div className="grid h-[3.5rem] grid-cols-2 gap-px overflow-hidden rounded-md border border-line bg-line">
        <ThemeSwatchSurface theme="light" />
        <ThemeSwatchSurface theme="dark" />
      </div>
    )
  }
  return (
    <div className="h-[3.5rem] overflow-hidden rounded-md border border-line">
      <ThemeSwatchSurface theme={theme} />
    </div>
  )
}

function ThemeSwatchSurface({ theme }: { theme: Exclude<Theme, 'system'> }) {
  const dark = themeIsDark(theme, false)
  return (
    <div data-theme={theme} className={`h-full ${dark ? 'dark' : ''}`}>
      <div className="flex h-full flex-col bg-surface">
        <div className="flex h-3 shrink-0 items-center gap-0.5 border-b border-line bg-surface-2 px-1.5">
          <span className="h-1 w-1 rounded-full bg-muted-2" />
          <span className="h-1 w-1 rounded-full bg-muted-2" />
          <span className="ml-auto h-1 w-5 rounded-full bg-surface-3" />
        </div>
        <div className="min-h-0 flex-1 px-1.5 py-1">
          <div className="flex items-center gap-1 border-b border-row-line py-[0.125rem]">
            <span className="h-1.5 w-1.5 rounded-[1px] bg-amber-400" />
            <span className="h-1 w-7 rounded-full bg-fg/60" />
            <span className="ml-auto h-1 w-3 rounded-full bg-muted-2/70" />
          </div>
          <div className="my-[0.125rem] flex items-center gap-1 rounded-[2px] bg-sel px-1 py-[0.125rem]">
            <span className="h-1.5 w-1.5 rounded-[1px] bg-blue-400" />
            <span className="h-1 w-9 rounded-full bg-fg-strong/80" />
          </div>
          <div className="flex items-center gap-1 border-b border-row-line py-[0.125rem]">
            <span className="h-1.5 w-1.5 rounded-[1px] bg-sky-400" />
            <span className="h-1 w-6 rounded-full bg-fg/60" />
          </div>
        </div>
      </div>
    </div>
  )
}

function ActiveCheck() {
  return (
    <svg aria-hidden viewBox="0 0 16 16" className="ml-auto h-3 w-3 shrink-0 text-blue-500">
      <path
        d="m3.5 8.5 3 3 6-7"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.75"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  )
}

function ThemeCard({
  theme,
  label,
  active,
  onSelect,
}: {
  theme: Theme
  label: string
  active: boolean
  onSelect: () => void
}) {
  return (
    <button
      type="button"
      aria-pressed={active}
      title={label}
      onClick={onSelect}
      className={`rounded-lg border p-1.5 text-left focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue-400 ${
        active ? 'border-blue-400 bg-surface-3/60' : 'border-line hover:border-line-strong hover:bg-surface-2'
      }`}
    >
      <ThemeSwatch theme={theme} />
      <span className="mt-1.5 flex items-center gap-1 px-0.5">
        <span className={`truncate text-[0.6875rem] ${active ? 'font-medium text-fg-strong' : 'text-muted'}`}>
          {label}
        </span>
        {active && <ActiveCheck />}
      </span>
    </button>
  )
}

/** One region's family (empty = inherit) and size; the list select is the shared datalist. */
function RegionFontRow({
  label,
  family,
  size,
  inheritHint,
  onChangeFamily,
  onChangeSize,
}: {
  label: string
  family: string
  size: number
  inheritHint: string
  onChangeFamily: (family: string) => void
  onChangeSize: (size: number) => void
}) {
  return (
    <div className="flex items-center gap-2">
      <span className="w-14 shrink-0 text-[0.71875rem] text-fg">{label}</span>
      <input
        list="font-family-options"
        value={family}
        placeholder={inheritHint}
        aria-label={label}
        onChange={(ev) => onChangeFamily(ev.target.value)}
        className="h-6 min-w-0 flex-1 rounded border border-line bg-surface-2 px-1.5 text-[0.6875rem] outline-none placeholder:text-muted-2 focus:border-blue-400 focus:bg-surface"
      />
      <input
        type="range"
        min={REGION_FONT_SIZE_MIN}
        max={FONT_SIZE_MAX}
        step={1}
        value={size}
        aria-label={label}
        onChange={(ev) => onChangeSize(Number(ev.target.value))}
        className="h-4 w-[5rem] shrink-0 cursor-pointer accent-blue-600"
      />
      <span className="tabular w-9 shrink-0 text-right text-[0.71875rem] text-muted">{size} px</span>
    </div>
  )
}

export function SettingsModal({ settings, lang, onChange, onOpenLocation, onClose }: Props) {
  const t = makeT(lang)
  const panelRef = useRef<HTMLDivElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)
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
  const [exporting, setExporting] = useState(false)
  const [exportNote, setExportNote] = useState('')
  // The select only holds the listed families; arbitrary installed families keep the
  // custom row open with the typed value, which is what `fontFamily` stores.
  const [customFont, setCustomFont] = useState(!FONT_FAMILIES.includes(settings.fontFamily))
  const [customDraft, setCustomDraft] = useState(
    FONT_FAMILIES.includes(settings.fontFamily) ? '' : settings.fontFamily,
  )
  const fontSize = clampFontSize(settings.fontSize)
  const tabFontSize = clampRegionFontSize(settings.tabFontSize, DEFAULT_TAB_FONT_SIZE)
  const listFontSize = clampRegionFontSize(settings.listFontSize, DEFAULT_LIST_FONT_SIZE)
  const navFontSize = clampRegionFontSize(settings.navFontSize, DEFAULT_NAV_FONT_SIZE)

  const keys = useMemo(() => resolveShortcuts(settings.shortcuts), [settings.shortcuts])
  const needle = query.trim().toLowerCase()

  // Section-level search index: every visible label is covered, so "gpu" or "docx" lands
  // on the right page without the user knowing the section names.
  const searchIndex: Record<SectionId, string> = {
    general: [
      t('settings.general'),
      t('settings.appearance'),
      t('settings.theme'),
      ...THEMES.map((entry) => t(entry.label)),
      t('settings.themeSystemHint'),
      t('settings.themeLightGroup'),
      t('settings.themeDarkGroup'),
      t('settings.font'),
      t('settings.fontSize'),
      t('settings.fontCustom'),
      t('settings.fontRegionsHint'),
      t('settings.fontTab'),
      t('settings.fontList'),
      t('settings.fontNav'),
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
    diagnostics: [
      t('settings.diagnostics'),
      t('settings.logLevel'),
      t('settings.logOpen'),
      t('settings.logExport'),
    ].join(' '),
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

  // The modal owns the keyboard while it is open: focus moves inside and Tab cycles
  // within the panel, so pane shortcuts cannot act on the list behind it.
  useEffect(() => {
    searchRef.current?.focus()
    const onTab = (ev: KeyboardEvent) => {
      if (ev.key !== 'Tab') return
      const panel = panelRef.current
      if (!panel) return
      const focusable = Array.from(
        panel.querySelectorAll<HTMLElement>(
          'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
        ),
      ).filter((el) => el.offsetParent !== null)
      if (!focusable.length) return
      const first = focusable[0]
      const last = focusable[focusable.length - 1]
      const active = document.activeElement
      if (ev.shiftKey && active === first) {
        ev.preventDefault()
        last.focus()
      } else if (!ev.shiftKey && active === last) {
        ev.preventDefault()
        first.focus()
      } else if (!panel.contains(active)) {
        ev.preventDefault()
        first.focus()
      }
    }
    window.addEventListener('keydown', onTab)
    return () => window.removeEventListener('keydown', onTab)
  }, [])

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
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-label={t('settings.title')}
        className="flex h-[37.5rem] max-h-[85vh] w-[47.5rem] max-w-full flex-col overflow-hidden rounded-xl border border-line bg-surface shadow-2xl"
      >
        <header className="flex items-center gap-3 border-b border-line px-4 py-3">
          <h1 className="text-[0.8125rem] font-semibold text-fg-strong">{t('settings.title')}</h1>
          <input
            ref={searchRef}
            value={query}
            onChange={(ev) => setQuery(ev.target.value)}
            placeholder={t('settings.searchPh')}
            data-settings-search
            className="ml-auto h-6 w-[12.5rem] rounded border border-line bg-surface-2 px-2 text-[0.6875rem] outline-none placeholder:text-muted-2 focus:border-blue-400 focus:bg-surface"
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
            <nav className="w-[8.5rem] shrink-0 space-y-0.5 overflow-y-auto border-r border-line bg-surface-2 p-1.5" data-settings-nav>
              {SECTIONS.map((entry) => (
                <button
                  key={entry.id}
                  type="button"
                  onClick={() => setSection(entry.id)}
                  className={`w-full rounded px-2 py-1.5 text-left text-[0.71875rem] ${
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
            {noMatch && <div className="text-[0.71875rem] text-muted-2">{t('settings.noMatch')}</div>}
          <section className={sectionClass('general')}>
            <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">{t('settings.appearance')}</h2>
            <div className="rounded-lg border border-line px-3 py-2.5">
              <span className="text-[0.75rem] text-fg">{t('settings.theme')}</span>
              <button
                type="button"
                aria-pressed={settings.theme === 'system'}
                onClick={() => onChange({ ...settings, theme: 'system' })}
                className={`mt-2 flex w-full items-center gap-3 rounded-lg border p-1.5 text-left focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue-400 ${
                  settings.theme === 'system'
                    ? 'border-blue-400 bg-surface-3/60'
                    : 'border-line hover:border-line-strong hover:bg-surface-2'
                }`}
              >
                <div className="w-[8.5rem] shrink-0">
                  <ThemeSwatch theme="system" />
                </div>
                <div className="min-w-0 flex-1">
                  <div
                    className={`text-[0.6875rem] ${
                      settings.theme === 'system' ? 'font-medium text-fg-strong' : 'text-muted'
                    }`}
                  >
                    {t('settings.themeSystem')}
                  </div>
                  <div className="mt-0.5 truncate text-[0.625rem] text-muted-2">
                    {t('settings.themeSystemHint')}
                  </div>
                </div>
                {settings.theme === 'system' && <ActiveCheck />}
              </button>

              {(
                [
                  ['settings.themeLightGroup', false],
                  ['settings.themeDarkGroup', true],
                ] as const
              ).map(([label, dark]) => (
                <div key={label} className="mt-3">
                  <div className="mb-1.5 text-[0.625rem] font-medium tracking-wide text-muted-2">
                    {t(label)}
                  </div>
                  <div className="grid grid-cols-[repeat(auto-fill,minmax(9rem,1fr))] gap-2">
                    {THEMES.filter((entry) => entry.dark === dark).map((entry) => (
                      <ThemeCard
                        key={entry.id}
                        theme={entry.id}
                        label={t(entry.label)}
                        active={settings.theme === entry.id}
                        onSelect={() => onChange({ ...settings, theme: entry.id })}
                      />
                    ))}
                  </div>
                </div>
              ))}
            </div>
          </section>

          <section className={sectionClass('general')}>
            <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">{t('settings.typography')}</h2>
            <div className="space-y-2">
              <div className="flex items-center justify-between gap-3 rounded-lg border border-line px-3 py-2">
                <div className="min-w-0">
                  <div className="text-[0.75rem] text-fg">{t('settings.font')}</div>
                  <div className="text-[0.65625rem] text-muted-2">{t('settings.fontHint')}</div>
                </div>
                <div className="flex shrink-0 items-center gap-1">
                  <select
                    value={customFont ? '__custom' : settings.fontFamily}
                    onChange={(ev) => {
                      const value = ev.target.value
                      if (value === '__custom') {
                        setCustomFont(true)
                        setCustomDraft('')
                        return
                      }
                      setCustomFont(false)
                      onChange({ ...settings, fontFamily: value })
                    }}
                    aria-label={t('settings.font')}
                    className="h-6 w-[9.5rem] rounded border border-line bg-surface-2 px-1.5 text-[0.71875rem] text-fg outline-none focus:border-blue-400 focus:bg-surface"
                  >
                    {FONT_FAMILIES.map((family) => (
                      <option key={family} value={family}>
                        {family}
                      </option>
                    ))}
                    <option value="__custom">{t('settings.fontCustom')}</option>
                  </select>
                  {customFont && (
                    <input
                      value={customDraft}
                      onChange={(ev) => {
                        setCustomDraft(ev.target.value)
                        onChange({ ...settings, fontFamily: ev.target.value.trim() || DEFAULT_FONT_FAMILY })
                      }}
                      placeholder={t('settings.fontCustomPh')}
                      aria-label={t('settings.fontCustomPh')}
                      className="h-6 w-[9.5rem] rounded border border-line bg-surface-2 px-1.5 text-[0.71875rem] outline-none placeholder:text-muted-2 focus:border-blue-400 focus:bg-surface"
                    />
                  )}
                </div>
              </div>
              <div className="flex items-center justify-between gap-3 rounded-lg border border-line px-3 py-2">
                <div className="min-w-0">
                  <div className="text-[0.75rem] text-fg">{t('settings.fontSize')}</div>
                  <div className="text-[0.65625rem] text-muted-2">{t('settings.fontSizeHint')}</div>
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  <input
                    type="range"
                    min={FONT_SIZE_MIN}
                    max={FONT_SIZE_MAX}
                    step={1}
                    value={fontSize}
                    onChange={(ev) => onChange({ ...settings, fontSize: Number(ev.target.value) })}
                    aria-label={t('settings.fontSize')}
                    className="h-4 w-[7rem] cursor-pointer accent-blue-600"
                  />
                  <span className="tabular w-9 text-right text-[0.71875rem] text-muted">{fontSize} px</span>
                </div>
              </div>
              <div className="space-y-1.5 rounded-lg border border-line px-3 py-2">
                <div className="text-[0.65625rem] text-muted-2">{t('settings.fontRegionsHint')}</div>
                <RegionFontRow
                  label={t('settings.fontTab')}
                  family={settings.tabFontFamily}
                  size={tabFontSize}
                  inheritHint={t('settings.fontInherit')}
                  onChangeFamily={(family) => onChange({ ...settings, tabFontFamily: family })}
                  onChangeSize={(size) => onChange({ ...settings, tabFontSize: size })}
                />
                <RegionFontRow
                  label={t('settings.fontList')}
                  family={settings.listFontFamily}
                  size={listFontSize}
                  inheritHint={t('settings.fontInherit')}
                  onChangeFamily={(family) => onChange({ ...settings, listFontFamily: family })}
                  onChangeSize={(size) => onChange({ ...settings, listFontSize: size })}
                />
                <RegionFontRow
                  label={t('settings.fontNav')}
                  family={settings.navFontFamily}
                  size={navFontSize}
                  inheritHint={t('settings.fontInherit')}
                  onChangeFamily={(family) => onChange({ ...settings, navFontFamily: family })}
                  onChangeSize={(size) => onChange({ ...settings, navFontSize: size })}
                />
                <datalist id="font-family-options">
                  {FONT_FAMILIES.map((family) => (
                    <option key={family} value={family} />
                  ))}
                </datalist>
              </div>
            </div>
          </section>

          <section className={sectionClass('general')}>
            <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">{t('settings.language')}</h2>
            <div className="flex items-center justify-between rounded-lg border border-line px-3 py-2">
              <span className="text-[0.75rem] text-fg">Language / 语言</span>
              <div className="flex gap-0.5 rounded-md bg-surface-3 p-0.5">
                {(['zh-CN', 'en'] as Lang[]).map((code) => (
                  <button
                    key={code}
                    type="button"
                    onClick={() => onChange({ ...settings, language: code })}
                    className={`rounded px-2.5 py-1 text-[0.71875rem] ${
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
            <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">{t('settings.behavior')}</h2>
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
                label={t('settings.notifyOps')}
                hint={t('settings.notifyOpsHint')}
                value={settings.notifyOps}
                onChange={(value) => onChange({ ...settings, notifyOps: value })}
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
            <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">{t('settings.shortcuts')}</h2>
            <div className="space-y-2" data-shortcuts>
              <div className="text-[0.65625rem] text-muted-2">{t('settings.shortcutsHint')}</div>
              {(['app', 'pane'] as const).map((scope) => (
                <div key={scope} className="overflow-hidden rounded-lg border border-line">
                  <div className="border-b border-line bg-surface-2 px-3 py-1 text-[0.625rem] font-semibold uppercase tracking-wider text-muted-2">
                    {scope === 'app' ? t('settings.shortcutApp') : t('settings.shortcutPane')}
                  </div>
                  {SHORTCUTS.filter((shortcut) => shortcut.scope === scope).map((shortcut) => {
                    const overridden = !sameBinding(keys[shortcut.id], DEFAULT_BINDINGS[shortcut.id])
                    return (
                      <div
                        key={shortcut.id}
                        className="flex items-center justify-between gap-2 border-b border-line px-3 py-1.5 text-[0.71875rem] last:border-b-0"
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
                            className={`min-w-[5.75rem] rounded border px-2 py-0.5 font-mono text-[0.6875rem] ${
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
                              className="rounded px-1 py-0.5 text-[0.65625rem] text-muted-2 hover:bg-surface-3 hover:text-fg"
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
                <span className="min-h-[0.875rem] text-[0.65625rem] text-red-500">{conflict ?? ''}</span>
                <button
                  type="button"
                  onClick={() => onChange({ ...settings, shortcuts: [] })}
                  disabled={!settings.shortcuts.length}
                  className="shrink-0 rounded border border-line px-2 py-0.5 text-[0.6875rem] text-fg hover:bg-surface-3 disabled:opacity-40"
                >
                  {t('settings.shortcutResetAll')}
                </button>
              </div>
            </div>
          </section>

          <section className={sectionClass('graphics')}>
            <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">{t('settings.graphics')}</h2>
            <div data-gpu-section>
              <ToggleRow
                label={t('settings.gpuAccel')}
                hint={t('settings.gpuAccelHint')}
                value={settings.gpuAccel}
                onChange={(value) => onChange({ ...settings, gpuAccel: value })}
              />
              <div className="mt-1.5 flex items-center justify-between gap-2 rounded-lg border border-line px-3 py-2">
                <span className="text-[0.65625rem] text-muted-2">{t('settings.gpuRestartHint')}</span>
                <button
                  type="button"
                  onClick={() => {
                    // The debounced save may not have run yet; persist the latest snapshot
                    // before the process restarts.
                    void settingsSave(settings)
                      .catch(() => undefined)
                      .then(() => restartApp().catch(() => undefined))
                  }}
                  className="shrink-0 rounded-md border border-line px-2 py-1 text-[0.6875rem] text-fg hover:bg-surface-3"
                >
                  {t('settings.restartNow')}
                </button>
              </div>
            </div>
          </section>

          <section className={sectionClass('quickAccess')}>
            <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">{t('settings.quickAccess')}</h2>
            <div className="space-y-1.5 rounded-lg border border-line px-3 py-2" data-quick-access>
              <div className="pb-1 text-[0.65625rem] text-muted-2">{t('settings.quickAccessHint')}</div>
              {visibleBuiltins.map((place) => (
                <div key={place.key} className="flex items-center justify-between gap-2 text-[0.71875rem]">
                  <span className="truncate text-fg">{place.label}</span>
                  <div className="flex shrink-0 items-center gap-2">
                    <span className="text-[0.625rem] uppercase tracking-wide text-muted-2">
                      {t('settings.placeBuiltin')}
                    </span>
                    <button
                      type="button"
                      onClick={() =>
                        onChange({ ...settings, hiddenPlaces: [...settings.hiddenPlaces, place.key] })
                      }
                      className="rounded px-1.5 py-0.5 text-[0.6875rem] text-muted hover:bg-surface-3 hover:text-red-500"
                    >
                      {t('settings.placeDelete')}
                    </button>
                  </div>
                </div>
              ))}
              {settings.customPlaces.map((place) => (
                <div key={place.path} className="flex items-center justify-between gap-2 text-[0.71875rem]">
                  <span className="min-w-0">
                    <span className="text-fg">{place.name}</span>
                    <span className="ml-2 truncate text-[0.65625rem] text-muted-2" title={place.path}>
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
                    className="shrink-0 rounded px-1.5 py-0.5 text-[0.6875rem] text-muted hover:bg-surface-3 hover:text-red-500"
                  >
                    {t('settings.placeDelete')}
                  </button>
                </div>
              ))}
              {settings.hiddenPlaces.length > 0 && (
                <button
                  type="button"
                  onClick={() => onChange({ ...settings, hiddenPlaces: [] })}
                  className="rounded px-1.5 py-0.5 text-[0.6875rem] text-blue-500 hover:bg-surface-3"
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
                  className="h-6 w-[6rem] rounded border border-line bg-surface-2 px-1.5 text-[0.6875rem] outline-none placeholder:text-muted-2 focus:border-blue-400 focus:bg-surface"
                />
                <input
                  value={placePath}
                  onChange={(ev) => setPlacePath(ev.target.value)}
                  onKeyDown={(ev) => {
                    if (ev.key === 'Enter') addPlace()
                  }}
                  placeholder={t('settings.placePathPh')}
                  data-place-path
                  className="h-6 min-w-0 flex-1 rounded border border-line bg-surface-2 px-1.5 text-[0.6875rem] outline-none placeholder:text-muted-2 focus:border-blue-400 focus:bg-surface"
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
                  className="h-6 shrink-0 rounded border border-line px-1.5 text-[0.6875rem] text-fg hover:bg-surface-3"
                >
                  {t('settings.placeBrowse')}
                </button>
                <button
                  type="button"
                  onClick={addPlace}
                  disabled={!placePath.trim()}
                  className="h-6 shrink-0 rounded bg-blue-600 px-2 text-[0.6875rem] text-white hover:bg-blue-500 disabled:opacity-40"
                >
                  {t('settings.placeAdd')}
                </button>
              </div>
            </div>
          </section>

          <section className={sectionClass('preview')}>
            <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">
              {t('settings.externalPreview')}
            </h2>
            <div className="space-y-1.5 rounded-lg border border-line px-3 py-2" data-external-preview>
              <div className="pb-1 text-[0.65625rem] text-muted-2">{t('settings.externalPreviewHint')}</div>
              <div className="pb-1 text-[0.65625rem] text-muted-2">
                {t('settings.externalKey', { keys: formatBinding(keys.externalPreview) })}
              </div>
              {settings.externalPreview.map((rule) => (
                <div key={rule.ext} className="flex items-center justify-between gap-2 text-[0.71875rem]">
                  <span className="min-w-0">
                    <span className="font-mono text-fg">.{rule.ext}</span>
                    <span className="ml-2 truncate font-mono text-[0.65625rem] text-muted-2" title={rule.command}>
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
                    className="shrink-0 rounded px-1.5 py-0.5 text-[0.6875rem] text-muted hover:bg-surface-3 hover:text-red-500"
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
                  className="h-6 w-[4.5rem] shrink-0 rounded border border-line bg-surface-2 px-1.5 text-[0.6875rem] outline-none placeholder:text-muted-2 focus:border-blue-400 focus:bg-surface"
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
                  className="h-6 min-w-0 flex-1 rounded border border-line bg-surface-2 px-1.5 text-[0.6875rem] outline-none placeholder:text-muted-2 focus:border-blue-400 focus:bg-surface"
                />
                <button
                  type="button"
                  onClick={addExternal}
                  disabled={!externalExt.trim() || !externalCommand.trim()}
                  className="h-6 shrink-0 rounded bg-blue-600 px-2 text-[0.6875rem] text-white hover:bg-blue-500 disabled:opacity-40"
                >
                  {t('settings.placeAdd')}
                </button>
              </div>
            </div>
          </section>

          <section className={sectionClass('config')}>
            <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">{t('settings.configDir')}</h2>
            <div className="flex items-center gap-2 rounded-lg border border-line px-3 py-2">
              <span className="min-w-0 flex-1 truncate font-mono text-[0.6875rem] text-muted" title={configDir} data-config-dir>
                {configDir || '…'}
              </span>
              <button
                type="button"
                disabled={!configDir}
                onClick={() => configDir && onOpenLocation(configDir)}
                className="shrink-0 rounded-md border border-line px-2 py-1 text-[0.6875rem] text-fg hover:bg-surface-3 disabled:opacity-40"
              >
                {t('settings.configOpen')}
              </button>
            </div>
          </section>

          <section className={sectionClass('updates')}>
            <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">{t('settings.updates')}</h2>
            <div className="space-y-2 rounded-lg border border-line px-3 py-2" data-update-section>
              <div className="flex items-center justify-between gap-2">
                <div className="min-w-0">
                  <div className="text-[0.75rem] text-fg">
                    v{info?.version ?? update?.current ?? '…'}
                    <span className="ml-2 text-[0.65625rem] text-muted-2">
                      {update?.flavor === 'installer'
                        ? t('settings.updateFlavorInstaller')
                        : t('settings.updateFlavorPortable')}
                    </span>
                  </div>
                  <div className="truncate text-[0.65625rem] text-muted-2" title={updateDetail} data-update-status>
                    {updateMessage}
                    {checkedAt && <span className="ml-1 text-muted-2">· {t('settings.updateLastCheck', { time: checkedAt })}</span>}
                  </div>
                </div>
                <button
                  type="button"
                  onClick={runUpdateCheck}
                  disabled={checking || installing}
                  data-update-check
                  className="shrink-0 rounded-md border border-line px-2 py-1 text-[0.6875rem] text-fg hover:bg-surface-3 disabled:opacity-40"
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
                  className="w-full rounded-md bg-blue-600 px-2 py-1 text-[0.71875rem] text-white hover:bg-blue-500 disabled:opacity-50"
                >
                  {installing ? t('settings.updateInstalling') : t('settings.updateInstall')}
                </button>
              )}
              {updateError && <div className="text-[0.65625rem] text-red-500">{updateError}</div>}
              <div className="flex items-center justify-between gap-2 border-t border-line pt-2">
                <span className="text-[0.75rem] text-fg">{t('settings.updateInterval')}</span>
                <select
                  value={settings.updateIntervalHours}
                  onChange={(ev) => onChange({ ...settings, updateIntervalHours: Number(ev.target.value) })}
                  data-update-interval
                  className="h-6 rounded border border-line bg-surface-2 px-1 text-[0.6875rem] text-fg outline-none focus:border-blue-400"
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

          <section className={sectionClass('diagnostics')}>
            <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">
              {t('settings.diagnostics')}
            </h2>
            <div className="space-y-2 rounded-lg border border-line px-3 py-2" data-diagnostics>
              <div className="flex items-center justify-between gap-2">
                <div className="min-w-0">
                  <div className="text-[0.71875rem] text-fg">{t('settings.logLevel')}</div>
                  <div className="text-[0.625rem] text-muted-2">{t('settings.logLevelHint')}</div>
                </div>
                <select
                  value={settings.logLevel}
                  onChange={(ev) => onChange({ ...settings, logLevel: ev.target.value as LogLevel })}
                  data-log-level
                  className="h-6 shrink-0 rounded border border-line bg-surface-2 px-1 text-[0.6875rem] text-fg outline-none focus:border-blue-400"
                >
                  {LOG_LEVELS.map((level) => (
                    <option key={level} value={level}>
                      {t(('settings.logLevel.' + level) as Key)}
                    </option>
                  ))}
                </select>
              </div>
              <div className="flex items-center gap-2 border-t border-line pt-2">
                <button
                  type="button"
                  onClick={() =>
                    void logDirPath()
                      .then((dir) => (dir ? onOpenLocation(dir) : undefined))
                      .catch(() => undefined)
                  }
                  className="shrink-0 rounded border border-line px-2 py-0.5 text-[0.6875rem] text-fg hover:bg-surface-3"
                >
                  {t('settings.logOpen')}
                </button>
                <button
                  type="button"
                  disabled={exporting}
                  onClick={() => {
                    setExporting(true)
                    setExportNote('')
                    void pickFolder(t('settings.logExportPick'), '')
                      .then((dir) => (dir ? diagnosticsExport(dir) : null))
                      .then((path) => {
                        if (path) setExportNote(t('settings.logExported', { path }))
                      })
                      .catch((err: unknown) => {
                        setExportNote(`${t('settings.logExportFailed')}: ${ipcErrorText(err)}`)
                      })
                      .finally(() => setExporting(false))
                  }}
                  className="shrink-0 rounded border border-line px-2 py-0.5 text-[0.6875rem] text-fg hover:bg-surface-3 disabled:opacity-40"
                >
                  {t('settings.logExport')}
                </button>
                <span
                  className="min-w-0 flex-1 truncate text-[0.625rem] text-muted-2"
                  title={exportNote}
                >
                  {exportNote}
                </span>
              </div>
            </div>
          </section>

          <section className={sectionClass('about')}>
            <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">{t('settings.about')}</h2>
            <div className="rounded-lg border border-line px-3 py-2 text-[0.71875rem]">
              <div className="flex justify-between py-0.5">
                <span className="text-muted">{t('settings.version')}</span>
                <span className="font-mono text-fg">{info?.version ?? '…'}</span>
              </div>
              <div className="mt-2 border-t border-line pt-2">
                <div className="pb-1 text-[0.65625rem] text-muted-2">{t('settings.dependencies')}</div>
                {(['rust', 'npm'] as const).map((kind) => {
                  const rows = (info?.dependencies ?? []).filter((dep) => dep.kind === kind)
                  if (!rows.length) return null
                  return (
                    <div key={kind} className="mb-2 last:mb-0">
                      <div className="pb-0.5 text-[0.59375rem] font-semibold uppercase tracking-wider text-muted-2">
                        {kind}
                      </div>
                      <div className="grid grid-cols-[minmax(0,1fr)_4.5rem_8rem] items-baseline gap-x-3 gap-y-1">
                        {rows.map((dep) => (
                          <div key={dep.name} className="contents">
                            <span className="truncate font-mono text-[0.6875rem] text-fg" title={dep.name}>
                              {dep.name}
                            </span>
                            <span className="text-right font-mono text-[0.6875rem] tabular text-muted">
                              {dep.version}
                            </span>
                            <span className="truncate text-right text-[0.65625rem] text-muted-2" title={dep.license}>
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
            className="rounded-md border border-line px-3 py-1 text-[0.71875rem] text-fg hover:bg-surface-3"
          >
            {t('settings.close')}
          </button>
        </footer>
      </div>
    </div>
  )
}

import { useEffect, useMemo, useRef, useState } from 'react'
import { appInfo, configPath, everythingStatus } from './api'
import { makeT } from './i18n'
import { SHORTCUTS, resolveShortcuts } from './shortcuts'
import type { AppInfo, EverythingStatus, Lang } from './types'
import { THEMES } from './themes'
import { ICON_THEMES } from './iconThemes'
import type { Settings } from './persist'
import { SECTIONS, type SectionId } from './components/settings/SettingsShared'
import { GeneralSection } from './components/settings/GeneralSection'
import { ColumnsSection, ConfigSection, GraphicsSection } from './components/settings/OtherSections'
import { ShortcutsSection } from './components/settings/ShortcutsSection'
import { IntegrationSection } from './components/settings/IntegrationSection'
import { QuickAccessSection } from './components/settings/QuickAccessSection'
import { ProtectionSection } from './components/settings/ProtectionSection'
import { ColorRulesSection } from './components/settings/ColorRulesSection'
import { ExternalPreviewSection } from './components/settings/ExternalPreviewSection'
import { UpdatesSection } from './components/settings/UpdatesSection'
import { DiagnosticsSection } from './components/settings/DiagnosticsSection'
import { AboutSection, DependenciesSection } from './components/settings/AboutSection'

type Props = {
  settings: Settings
  lang: Lang
  onChange: (next: Settings) => void
  /** Opens a folder inside FlowDir itself; the settings modal closes first. */
  onOpenLocation: (path: string) => void
  onClose: () => void
}

export function SettingsModal({ settings, lang, onChange, onOpenLocation, onClose }: Props) {
  const t = makeT(lang)
  const panelRef = useRef<HTMLDivElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)
  const [info, setInfo] = useState<AppInfo | null>(null)
  const [everything, setEverything] = useState<EverythingStatus | null>(null)
  const [configDir, setConfigDir] = useState('')
  const [section, setSection] = useState<SectionId>('general')
  const [query, setQuery] = useState('')

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
      t('settings.iconSize'),
      t('settings.iconTheme'),
      ...ICON_THEMES.map((entry) => `${t(entry.label)} ${t(entry.hint)}`),
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
      t('settings.everythingOff'),
      // The remaining GeneralSection controls: without these the search claimed a match
      // did not exist while the setting was visible on the page.
      t('settings.restoreTabs'),
      t('settings.typography'),
      t('settings.closedTabLimit'),
      t('settings.closedTabLimitHint'),
      t('settings.undoLimit'),
      t('settings.undoLimitHint'),
      t('settings.middleClickNewTab'),
      t('settings.notifyOps'),
      t('settings.notifyOpsHint'),
      t('settings.dupSpeed'),
      t('settings.dupSpeedHint'),
      t('settings.dupConcurrency'),
      t('settings.dupConcurrencyHint'),
      t('settings.dupExclude'),
      t('settings.dupExcludeHint'),
      t('settings.fontHint'),
      t('settings.fontSizeHint'),
      t('settings.iconSizeHint'),
      t('settings.themeSystem'),
      // Merged tabs: list columns and graphics/performance.
      t('settings.columns'),
      t('settings.columnsHint'),
      t('list.size'),
      t('list.type'),
      t('list.mtime'),
      t('settings.graphics'),
      t('settings.gpuAccel'),
      t('settings.gpuAccelHint'),
      t('settings.reclaimMemory'),
      t('settings.reclaimMemoryHint'),
      t('settings.restartNow'),
    ].join(' '),
    colors: [
      t('settings.colorRules'),
      t('settings.colorRulesHint'),
      ...settings.colorRules.map((rule) => rule.pattern),
    ].join(' '),
    shortcuts: [
      t('settings.shortcutsIntegration'),
      t('settings.shortcuts'),
      ...SHORTCUTS.map((shortcut) => t(shortcut.label)),
      t('settings.integration'),
      t('settings.hotkey'),
      t('settings.contextMenu'),
      t('settings.associateFolders'),
      t('settings.autostart'),
      settings.globalHotkey,
    ].join(' '),
    quickAccess: [
      t('settings.quickAccessPreview'),
      t('settings.quickAccess'),
      ...settings.customPlaces.map((place) => place.name),
      t('settings.previewSection'),
      t('settings.externalPreview'),
      ...settings.externalPreview.map((rule) => rule.ext),
    ].join(' '),
    protection: [
      t('settings.protection'),
      t('settings.protectionHint'),
      t('settings.protectionBlock'),
      t('settings.protectionBlockHint'),
      t('settings.protectionConfirm'),
      t('settings.protectionConfirmHint'),
      t('settings.protectionAlways'),
      ...settings.protectBlock,
      ...settings.protectConfirm,
    ].join(' '),
    about: [
      t('settings.aboutUpdates'),
      t('settings.about'),
      t('settings.version'),
      t('settings.repository'),
      t('settings.profile'),
      t('settings.dependencies'),
      t('settings.dependenciesHint'),
      t('settings.updates'),
      t('settings.updateInterval'),
      t('settings.updateCheck'),
      t('settings.updateHistory'),
    ].join(' '),
    diagnostics: [
      t('settings.diagnosticsConfig'),
      t('settings.diagnostics'),
      t('settings.logLevel'),
      t('settings.logOpen'),
      t('settings.logExport'),
      t('settings.configDir'),
      t('settings.configOpen'),
    ].join(' '),
  }

  const isSectionVisible = (id: SectionId) => {
    if (needle) return searchIndex[id].toLowerCase().includes(needle)
    return section === id
  }

  const noMatch = needle !== '' && !SECTIONS.some((entry) => searchIndex[entry.id].toLowerCase().includes(needle))

  useEffect(() => {
    void appInfo().then(setInfo).catch(() => undefined)
    void everythingStatus().then(setEverything).catch(() => undefined)
    void configPath().then(setConfigDir).catch(() => undefined)
  }, [])

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
      const focusables = panel.querySelectorAll<HTMLElement>(
        'button:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])',
      )
      if (!focusables.length) return
      const first = focusables[0]
      const last = focusables[focusables.length - 1]
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

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/40 p-6 backdrop-blur-[2px]"
      onClick={(ev) => {
        if (ev.target === ev.currentTarget) onClose()
      }}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-label={t('app.settings')}
        className="flex h-[42rem] max-h-full w-[54rem] max-w-full flex-col overflow-hidden rounded-xl border border-line bg-surface text-fg shadow-2xl"
      >
        <header className="flex h-11 shrink-0 items-center justify-between border-b border-line px-4">
          <div className="flex items-center gap-2">
            <span className="text-[0.875rem] font-semibold text-fg-strong">{t('app.settings')}</span>
          </div>
          <div className="flex items-center gap-2">
            <div className="relative">
              <input
                ref={searchRef}
                value={query}
                onChange={(ev) => setQuery(ev.target.value)}
                placeholder={t('settings.searchPh')}
                className="h-7 w-48 rounded-md border border-line bg-surface-2 pl-7 pr-2 text-[0.75rem] outline-none placeholder:text-muted-2 focus:border-accent-soft focus:bg-surface"
              />
              <svg
                aria-hidden
                viewBox="0 0 16 16"
                className="pointer-events-none absolute left-2 top-2 h-3.5 w-3.5 text-muted-2"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.5"
              >
                <circle cx="7" cy="7" r="4.2" />
                <path d="m10.2 10.2 3 3" strokeLinecap="round" />
              </svg>
            </div>
            <button
              type="button"
              onClick={onClose}
              className="grid h-7 w-7 place-items-center rounded-md text-muted hover:bg-surface-3 hover:text-fg"
            >
              ×
            </button>
          </div>
        </header>

        <div className="flex min-h-0 flex-1">
          <nav
            aria-label={t('app.settings')}
            className="w-44 shrink-0 overflow-y-auto border-r border-line bg-surface-2 p-2"
          >
            <ul className="space-y-0.5">
              {SECTIONS.map((entry) => (
                <li key={entry.id}>
                  <button
                    type="button"
                    onClick={() => {
                      setSection(entry.id)
                      setQuery('')
                    }}
                    className={`w-full rounded px-2 py-1.5 text-left text-[0.71875rem] ${
                      section === entry.id
                        ? 'bg-surface font-medium text-fg-strong shadow-sm'
                        : 'text-muted hover:bg-surface-3 hover:text-fg'
                    }`}
                  >
                    {t(entry.label)}
                  </button>
                </li>
              ))}
            </ul>
          </nav>

          <div className="min-w-0 flex-1 space-y-5 overflow-y-auto px-4 py-4">
            {noMatch && <div className="text-[0.71875rem] text-muted-2">{t('settings.noMatch')}</div>}

            <GeneralSection
              settings={settings}
              onChange={onChange}
              t={t}
              visible={isSectionVisible('general')}
              everything={everything}
            />

            <ColumnsSection
              settings={settings}
              onChange={onChange}
              t={t}
              visible={isSectionVisible('general')}
            />

            <GraphicsSection
              settings={settings}
              onChange={onChange}
              t={t}
              visible={isSectionVisible('general')}
            />

            <ColorRulesSection
              settings={settings}
              onChange={onChange}
              t={t}
              visible={isSectionVisible('colors')}
            />

            <ShortcutsSection
              settings={settings}
              keys={keys}
              onChange={onChange}
              t={t}
              visible={isSectionVisible('shortcuts')}
            />

            <IntegrationSection
              settings={settings}
              onChange={onChange}
              t={t}
              visible={isSectionVisible('shortcuts')}
            />

            <QuickAccessSection
              settings={settings}
              onChange={onChange}
              t={t}
              visible={isSectionVisible('quickAccess')}
              configDir={configDir}
            />

            <ExternalPreviewSection
              settings={settings}
              keys={keys}
              onChange={onChange}
              t={t}
              visible={isSectionVisible('quickAccess')}
            />

            <ProtectionSection
              settings={settings}
              onChange={onChange}
              t={t}
              visible={isSectionVisible('protection')}
            />

            <AboutSection
              info={info}
              t={t}
              visible={isSectionVisible('about')}
            />

            <UpdatesSection
              settings={settings}
              info={info}
              lang={lang}
              onChange={onChange}
              t={t}
              visible={isSectionVisible('about')}
            />

            <DependenciesSection
              info={info}
              t={t}
              visible={isSectionVisible('about')}
            />

            <DiagnosticsSection
              settings={settings}
              onChange={onChange}
              onOpenLocation={onOpenLocation}
              t={t}
              visible={isSectionVisible('diagnostics')}
            />

            <ConfigSection
              configDir={configDir}
              onOpenLocation={onOpenLocation}
              t={t}
              visible={isSectionVisible('diagnostics')}
            />
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

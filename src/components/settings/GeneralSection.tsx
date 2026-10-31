import { useState } from 'react'
import type { Translate } from '../../i18n'
import type { Settings } from '../../persist'
import type { EverythingStatus, Lang } from '../../types'
import { THEMES } from '../../themes'
import {
  DEFAULT_FONT_FAMILY,
  DEFAULT_LIST_FONT_SIZE,
  DEFAULT_NAV_FONT_SIZE,
  DEFAULT_TAB_FONT_SIZE,
  FONT_FAMILIES,
  FONT_SIZE_MAX,
  FONT_SIZE_MIN,
  ICON_SIZE_MAX,
  ICON_SIZE_MIN,
  clampFontSize,
  clampIconSize,
  clampRegionFontSize,
} from '../../fonts'
import { ActiveCheck, RegionFontRow, ThemeCard, ThemeSwatch, ToggleRow } from './SettingsShared'
import { ICON_THEMES, type IconThemeId } from '../../iconThemes'

export type GeneralSectionProps = {
  settings: Settings
  onChange: (next: Settings) => void
  t: Translate
  visible: boolean
  everything: EverythingStatus | null
}

export function GeneralSection({ settings, onChange, t, visible, everything }: GeneralSectionProps) {
  const [customFont, setCustomFont] = useState(!FONT_FAMILIES.includes(settings.fontFamily))
  const [customDraft, setCustomDraft] = useState(
    FONT_FAMILIES.includes(settings.fontFamily) ? '' : settings.fontFamily,
  )

  const fontSize = clampFontSize(settings.fontSize)
  const tabFontSize = clampRegionFontSize(settings.tabFontSize, DEFAULT_TAB_FONT_SIZE)
  const listFontSize = clampRegionFontSize(settings.listFontSize, DEFAULT_LIST_FONT_SIZE)
  const navFontSize = clampRegionFontSize(settings.navFontSize, DEFAULT_NAV_FONT_SIZE)
  const iconSize = clampIconSize(settings.iconSize)

  if (!visible) return null

  return (
    <div className="space-y-5">
      <section>
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

      <section>
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
          <div className="flex items-center justify-between gap-3 rounded-lg border border-line px-3 py-2">
            <div className="min-w-0">
              <div className="text-[0.75rem] text-fg">{t('settings.iconSize')}</div>
              <div className="text-[0.65625rem] text-muted-2">{t('settings.iconSizeHint')}</div>
            </div>
            <div className="flex shrink-0 items-center gap-2">
              <input
                type="range"
                min={ICON_SIZE_MIN}
                max={ICON_SIZE_MAX}
                step={1}
                value={iconSize}
                onChange={(ev) => onChange({ ...settings, iconSize: Number(ev.target.value) })}
                aria-label={t('settings.iconSize')}
                className="h-4 w-[7rem] cursor-pointer accent-blue-600"
              />
              <span className="tabular w-9 text-right text-[0.71875rem] text-muted">{iconSize} px</span>
            </div>
          </div>
          <div className="flex items-center justify-between gap-3 rounded-lg border border-line px-3 py-2">
            <div className="min-w-0">
              <div className="text-[0.75rem] text-fg">{t('settings.iconTheme')}</div>
              <div className="truncate text-[0.65625rem] text-muted-2">
                {t(ICON_THEMES.find((entry) => entry.id === settings.iconTheme)?.hint ?? 'settings.iconThemeHint')}
              </div>
            </div>
            <select
              value={settings.iconTheme}
              onChange={(ev) => onChange({ ...settings, iconTheme: ev.target.value as IconThemeId })}
              aria-label={t('settings.iconTheme')}
              className="h-7 shrink-0 rounded border border-line bg-surface-2 px-1.5 text-[0.71875rem] text-fg outline-none focus:border-blue-400"
            >
              {ICON_THEMES.map((entry) => (
                <option key={entry.id} value={entry.id}>
                  {t(entry.label)}
                </option>
              ))}
            </select>
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

      <section>
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

      <section>
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
            label={t('settings.restoreTabs')}
            value={settings.restoreTabsOnStart}
            onChange={(value) => onChange({ ...settings, restoreTabsOnStart: value })}
          />
          <ToggleRow
            label={t('settings.middleClickNewTab')}
            value={settings.newTabOnMiddleClick}
            onChange={(value) => onChange({ ...settings, newTabOnMiddleClick: value })}
          />
          <div className="flex items-center justify-between gap-2">
            <div className="min-w-0">
              <div className="text-[0.71875rem] text-fg">{t('settings.closedTabLimit')}</div>
              <div className="text-[0.625rem] text-muted-2">{t('settings.closedTabLimitHint')}</div>
            </div>
            <input
              type="number"
              min={1}
              max={50}
              value={settings.closedTabLimit}
              onChange={(ev) => {
                const next = Number(ev.target.value)
                if (!Number.isFinite(next)) return
                onChange({ ...settings, closedTabLimit: Math.min(50, Math.max(1, Math.round(next))) })
              }}
              className="h-6 w-16 shrink-0 rounded border border-line bg-surface-2 px-1 text-right text-[0.6875rem] tabular text-fg outline-none focus:border-blue-400"
            />
          </div>
          <div className="flex items-center justify-between gap-2">
            <div className="min-w-0">
              <div className="text-[0.71875rem] text-fg">{t('settings.dupConcurrency')}</div>
              <div className="text-[0.625rem] text-muted-2">{t('settings.dupConcurrencyHint')}</div>
            </div>
            <input
              type="number"
              min={0}
              max={16}
              value={settings.dupConcurrency}
              onChange={(ev) => {
                const next = Number(ev.target.value)
                if (!Number.isFinite(next)) return
                onChange({ ...settings, dupConcurrency: Math.min(16, Math.max(0, Math.round(next))) })
              }}
              className="h-6 w-16 shrink-0 rounded border border-line bg-surface-2 px-1 text-right text-[0.6875rem] tabular text-fg outline-none focus:border-blue-400"
            />
          </div>
          <div className="flex items-center justify-between gap-2">
            <div className="min-w-0">
              <div className="text-[0.71875rem] text-fg">{t('settings.dupSpeed')}</div>
              <div className="text-[0.625rem] text-muted-2">{t('settings.dupSpeedHint')}</div>
            </div>
            <input
              type="number"
              min={0}
              max={4096}
              value={settings.dupMaxMbPerSec}
              onChange={(ev) => {
                const next = Number(ev.target.value)
                if (!Number.isFinite(next)) return
                onChange({ ...settings, dupMaxMbPerSec: Math.min(4096, Math.max(0, Math.round(next))) })
              }}
              className="h-6 w-16 shrink-0 rounded border border-line bg-surface-2 px-1 text-right text-[0.6875rem] tabular text-fg outline-none focus:border-blue-400"
            />
          </div>
          <div className="space-y-1">
            <div className="text-[0.71875rem] text-fg">{t('settings.dupExclude')}</div>
            <div className="text-[0.625rem] text-muted-2">{t('settings.dupExcludeHint')}</div>
            <input
              value={settings.dupExclude.join(', ')}
              onChange={(ev) =>
                onChange({
                  ...settings,
                  dupExclude: ev.target.value
                    .split(',')
                    .map((name) => name.trim())
                    .filter(Boolean),
                })
              }
              className="h-6 w-full rounded border border-line bg-surface-2 px-1 text-[0.6875rem] text-fg outline-none focus:border-blue-400"
            />
          </div>
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
          <div className="flex items-center justify-between gap-2">
            <div className="min-w-0">
              <div className="text-[0.71875rem] text-fg">{t('settings.undoLimit')}</div>
              <div className="text-[0.625rem] text-muted-2">{t('settings.undoLimitHint')}</div>
            </div>
            <input
              type="number"
              min={1}
              max={100}
              value={settings.undoLimit}
              onChange={(ev) => {
                const next = Number(ev.target.value)
                if (!Number.isFinite(next)) return
                onChange({ ...settings, undoLimit: Math.min(100, Math.max(1, Math.round(next))) })
              }}
              data-undo-limit
              className="h-6 w-16 shrink-0 rounded border border-line bg-surface-2 px-1 text-right text-[0.6875rem] tabular text-fg outline-none focus:border-blue-400"
            />
          </div>
        </div>
      </section>
    </div>
  )
}

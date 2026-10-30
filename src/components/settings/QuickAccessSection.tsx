import { useState } from 'react'
import type { Translate } from '../../i18n'
import type { Settings } from '../../persist'
import { pickFolder } from '../../api'

export type QuickAccessSectionProps = {
  settings: Settings
  onChange: (next: Settings) => void
  t: Translate
  visible: boolean
  configDir: string
}

export function QuickAccessSection({ settings, onChange, t, visible, configDir }: QuickAccessSectionProps) {
  const [placeName, setPlaceName] = useState('')
  const [placePath, setPlacePath] = useState('')

  const builtinPlaces: { key: string; label: string }[] = [
    { key: 'home', label: t('place.home') },
    { key: 'desktop', label: t('place.desktop') },
    { key: 'downloads', label: t('place.downloads') },
    { key: 'documents', label: t('place.documents') },
  ]
  const visibleBuiltins = builtinPlaces.filter((place) => !settings.hiddenPlaces.includes(place.key))

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

  if (!visible) return null

  return (
    <section>
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
                onClick={() => onChange({ ...settings, hiddenPlaces: [...settings.hiddenPlaces, place.key] })}
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
  )
}

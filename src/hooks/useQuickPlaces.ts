import { useCallback, useMemo, useState } from 'react'
import type { PointerEvent as ReactPointerEvent } from 'react'
import type { Place, Lang } from '../types'
import type { Settings } from '../persist'
import type { Translate } from '../i18n'
import type { QuickPlaceEntry } from './usePaneContextMenus'
import { baseName, pathKey } from '../paths'

export interface UseQuickPlacesOptions {
  places: Place[]
  settings: Settings
  updateSettings: (next: Settings) => void
  sidebarMin: number
  showToast: (text: string) => void
  t: Translate
  lang: Lang
}

export function useQuickPlaces({
  places,
  settings,
  updateSettings,
  sidebarMin,
  showToast,
  t,
  lang,
}: UseQuickPlacesOptions) {
  const [sidebarWidth, setSidebarWidth] = useState(settings.sidebarWidth)

  const toggleSidebar = useCallback(() => {
    updateSettings({ ...settings, sidebarOpen: !settings.sidebarOpen })
  }, [settings, updateSettings])

  const startSidebarResize = useCallback(
    (ev: ReactPointerEvent<HTMLDivElement>) => {
      ev.preventDefault()
      const startX = ev.clientX
      const startWidth = sidebarWidth
      let finalWidth = startWidth
      const move = (e: PointerEvent) => {
        const next = Math.max(0, startWidth + (e.clientX - startX))
        if (next < sidebarMin) {
          if (settings.sidebarOpen) updateSettings({ ...settings, sidebarOpen: false })
          return
        }
        if (!settings.sidebarOpen) updateSettings({ ...settings, sidebarOpen: true })
        finalWidth = Math.round(next)
        setSidebarWidth(finalWidth)
      }
      const up = () => {
        window.removeEventListener('pointermove', move)
        window.removeEventListener('pointerup', up)
        window.removeEventListener('pointercancel', up)
        if (finalWidth >= sidebarMin) updateSettings({ ...settings, sidebarWidth: finalWidth })
      }
      window.addEventListener('pointermove', move)
      window.addEventListener('pointerup', up)
      window.addEventListener('pointercancel', up)
    },
    [sidebarMin, sidebarWidth, settings, updateSettings],
  )

  const isPinnedPlace = useCallback(
    (targetPath: string) => {
      const key = pathKey(targetPath)
      return (
        places.some((place) => !settings.hiddenPlaces.includes(place.key) && pathKey(place.path) === key) ||
        settings.customPlaces.some((place) => pathKey(place.path) === key)
      )
    },
    [places, settings.customPlaces, settings.hiddenPlaces],
  )

  const togglePinPlace = useCallback(
    (targetPath: string) => {
      const key = pathKey(targetPath)
      const builtin = places.find((place) => pathKey(place.path) === key)
      if (builtin) {
        const hidden = settings.hiddenPlaces.includes(builtin.key)
        const next = hidden
          ? settings.hiddenPlaces.filter((k) => k !== builtin.key)
          : [...settings.hiddenPlaces, builtin.key]
        updateSettings({ ...settings, hiddenPlaces: next })
        showToast(hidden ? t('sidebar.pinned') : t('sidebar.unpinned'))
        return
      }
      const existing = settings.customPlaces.find((place) => pathKey(place.path) === key)
      if (existing) {
        updateSettings({
          ...settings,
          customPlaces: settings.customPlaces.filter((place) => pathKey(place.path) !== key),
        })
        showToast(t('sidebar.unpinned'))
        return
      }
      const name = baseName(targetPath) || targetPath
      updateSettings({
        ...settings,
        customPlaces: [...settings.customPlaces, { name, path: targetPath }],
      })
      showToast(t('sidebar.pinned'))
    },
    [places, settings, showToast, t, updateSettings],
  )

  const placeLabel = (key: string) => {
    switch (key) {
      case 'home':
        return t('place.home')
      case 'desktop':
        return t('place.desktop')
      case 'downloads':
        return t('place.downloads')
      case 'documents':
        return t('place.documents')
      default:
        return key
    }
  }

  const quickPlaces = useMemo<QuickPlaceEntry[]>(() => {
    const builtin = places
      .filter((place) => !settings.hiddenPlaces.includes(place.key))
      .map((place) => ({ key: place.key, label: placeLabel(place.key), path: place.path, custom: false }))
    const custom = settings.customPlaces.map((place) => ({
      key: `custom:${place.path}`,
      label: place.name || place.path,
      path: place.path,
      custom: true,
    }))
    return [...builtin, ...custom]
  }, [places, settings.hiddenPlaces, settings.customPlaces, lang])

  const removePlace = useCallback(
    (place: QuickPlaceEntry) => {
      if (place.custom) {
        const next = settings.customPlaces.filter((p) => p.path !== place.path)
        updateSettings({ ...settings, customPlaces: next })
      } else {
        const next = [...settings.hiddenPlaces, place.key]
        updateSettings({ ...settings, hiddenPlaces: next })
      }
    },
    [settings, updateSettings],
  )

  return {
    sidebarWidth,
    setSidebarWidth,
    toggleSidebar,
    startSidebarResize,
    isPinnedPlace,
    togglePinPlace,
    quickPlaces,
    removePlace,
  }
}

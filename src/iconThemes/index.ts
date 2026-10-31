import { useSyncExternalStore } from 'react'
import type { Key } from '../i18n'
import type { IconAssets, IconThemeId } from './catalog'

export type { IconThemeId } from './catalog'

/** Settings-page choices; the label/hint keys live in `locales`. */
export const ICON_THEMES: { id: IconThemeId; label: Key; hint: Key }[] = [
  { id: 'system', label: 'settings.iconThemeSystem', hint: 'settings.iconThemeSystemHint' },
  { id: 'flat', label: 'settings.iconThemeFlat', hint: 'settings.iconThemeFlatHint' },
  { id: 'material', label: 'settings.iconThemeMaterial', hint: 'settings.iconThemeMaterialHint' },
  { id: 'catppuccin', label: 'settings.iconThemeCatppuccin', hint: 'settings.iconThemeCatppuccinHint' },
]

export type IconThemeSnapshot = { id: IconThemeId; assets: IconAssets | null }

let themeId: IconThemeId = 'system'
let dark = false
let assets: IconAssets | null = null
let generation = 0
let snapshot: IconThemeSnapshot = { id: themeId, assets: null }
const listeners = new Set<() => void>()

function publish() {
  snapshot = { id: themeId, assets }
  for (const listener of listeners) listener()
}

function load(id: IconThemeId, isDark: boolean): Promise<{ default: IconAssets } | null> {
  if (id === 'material') return import('./material')
  if (id === 'catppuccin') {
    return isDark ? import('./catppuccinMocha') : import('./catppuccinLatte')
  }
  return Promise.resolve(null)
}

/**
 * Switches the bundled icon theme. `isDark` only drives Catppuccin, which ships a
 * palette per appearance; the other themes use one set for both.
 */
export function applyIconTheme(id: IconThemeId, isDark: boolean) {
  if (id === themeId && isDark === dark) return
  themeId = id
  dark = isDark
  const current = ++generation
  assets = null
  publish()
  void load(id, isDark)
    .then((module) => {
      if (current !== generation) return
      assets = module?.default ?? null
      publish()
    })
    .catch(() => {
      if (current !== generation) return
      assets = null
      publish()
    })
}

function subscribe(listener: () => void) {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

function snapshotOf() {
  return snapshot
}

/** Reactive view of the active theme; rows fall back to the built-in SVGs while loading. */
export function useIconTheme(): IconThemeSnapshot {
  return useSyncExternalStore(subscribe, snapshotOf)
}
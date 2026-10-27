import type { Key } from './i18n'
import type { Theme } from './types'

/**
 * One selectable palette. `dark` is null only for `system`, whose scheme follows the OS.
 * The ids, schemes and labels here mirror the palette blocks in `styles.css` and
 * `theme::palette` in `src-tauri/src/theme.rs`.
 */
export type ThemeDef = { id: Theme; label: Key; dark: boolean | null }

/** Picker order: automatic first, then light palettes, then dark palettes. */
export const THEMES: ThemeDef[] = [
  { id: 'system', label: 'settings.themeSystem', dark: null },
  { id: 'light', label: 'settings.themeLight', dark: false },
  { id: 'sepia', label: 'settings.themeSepia', dark: false },
  { id: 'mist', label: 'settings.themeMist', dark: false },
  { id: 'dark', label: 'settings.themeDark', dark: true },
  { id: 'midnight', label: 'settings.themeMidnight', dark: true },
  { id: 'graphite', label: 'settings.themeGraphite', dark: true },
  { id: 'nord', label: 'settings.themeNord', dark: true },
  { id: 'forest', label: 'settings.themeForest', dark: true },
]

/** Resolves whether a theme paints with dark tokens; `system` takes the OS answer. */
export function themeIsDark(theme: Theme, systemDark: boolean): boolean {
  const entry = THEMES.find((candidate) => candidate.id === theme)
  return entry?.dark ?? systemDark
}

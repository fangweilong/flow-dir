/** UI typography options shared by the settings page and the root CSS variables. */

export const BASE_FONT_SIZE = 13
export const FONT_SIZE_MIN = 12
export const FONT_SIZE_MAX = 20
/** Region text may go one step below the global floor: compact tab strips stay usable. */
export const REGION_FONT_SIZE_MIN = 10
export const DEFAULT_FONT_FAMILY = 'Segoe UI Variable Text'
export const DEFAULT_TAB_FONT_SIZE = 12
export const DEFAULT_LIST_FONT_SIZE = 13
export const DEFAULT_NAV_FONT_SIZE = 12

/** Common Windows families; any other installed family can be typed in. */
export const FONT_FAMILIES = [
  DEFAULT_FONT_FAMILY,
  'Segoe UI',
  'Microsoft YaHei UI',
  'Microsoft YaHei',
  'SimSun',
  'Consolas',
  'Arial',
]

const DEFAULT_STACK = `"${DEFAULT_FONT_FAMILY}", "Segoe UI", system-ui, sans-serif`

export function clampFontSize(size: number): number {
  if (!Number.isFinite(size)) return BASE_FONT_SIZE
  return Math.min(FONT_SIZE_MAX, Math.max(FONT_SIZE_MIN, Math.round(size)))
}

/** Same repair for per-region sizes, with the wider floor the compact surfaces allow. */
export function clampRegionFontSize(size: number, fallback: number): number {
  if (!Number.isFinite(size)) return fallback
  return Math.min(FONT_SIZE_MAX, Math.max(REGION_FONT_SIZE_MIN, Math.round(size)))
}

/**
 * Builds the CSS stack for one family. A stored family is a single name, so anything
 * that could terminate the property (quotes, separators, braces) falls back instead
 * of reaching the stylesheet. The backend applies the same rule on load.
 */
export function fontStack(family: string): string {
  const name = family.trim()
  const unsafe = /["'\\;{},]/.test(name) || Array.from(name).some((char) => char.charCodeAt(0) < 32)
  if (!name || name.length > 64 || unsafe || name === DEFAULT_FONT_FAMILY) return DEFAULT_STACK
  return `"${name}", ${DEFAULT_STACK}`
}

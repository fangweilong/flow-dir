/** UI typography options shared by the settings page and the root CSS variables. */

export const BASE_FONT_SIZE = 13
export const FONT_SIZE_MIN = 12
export const FONT_SIZE_MAX = 20
export const DEFAULT_FONT_FAMILY = 'Segoe UI Variable Text'

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

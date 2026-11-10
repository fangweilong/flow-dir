/** UI typography options shared by the settings page and the root CSS variables. */

import { systemFonts } from './api'

export const BASE_FONT_SIZE = 13
export const FONT_SIZE_MIN = 12
export const FONT_SIZE_MAX = 20
/** Region text may go one step below the global floor: compact tab strips stay usable. */
export const REGION_FONT_SIZE_MIN = 10
export const DEFAULT_FONT_FAMILY = 'Segoe UI Variable Text'
export const DEFAULT_TAB_FONT_SIZE = 12
export const DEFAULT_LIST_FONT_SIZE = 13
export const DEFAULT_NAV_FONT_SIZE = 12
export const DEFAULT_ICON_SIZE = 20
export const ICON_SIZE_MIN = 14
export const ICON_SIZE_MAX = 32
/** Curated families shown first; every other installed family is appended at runtime. */
export const FONT_FAMILIES = [
  DEFAULT_FONT_FAMILY,
  'Segoe UI',
  'Microsoft YaHei UI',
  'Microsoft YaHei',
  'SimSun',
  'Consolas',
  'Arial',
]

let installedFamilies: Promise<string[]> | null = null

/**
 * Installed families merged with the curated defaults. Enumerated once per session,
 * because the OS font table is static and the pickers remount with the settings modal.
 * A failed call keeps the static list so family selection still works without a backend.
 */
export function loadFontFamilies(): Promise<string[]> {
  installedFamilies ??= systemFonts()
    .then(mergeFontFamilies)
    .catch(() => {
      installedFamilies = null
      return FONT_FAMILIES
    })
  return installedFamilies
}

function mergeFontFamilies(names: string[]): string[] {
  const seen = new Set(FONT_FAMILIES.map((name) => name.toLowerCase()))
  const extra = names.filter((name) => {
    const key = name.toLowerCase()
    if (!name || seen.has(key)) return false
    seen.add(key)
    return true
  })
  extra.sort((left, right) => left.localeCompare(right))
  return [...FONT_FAMILIES, ...extra]
}

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

export function clampIconSize(size: number): number {
  if (!Number.isFinite(size)) return DEFAULT_ICON_SIZE
  return Math.min(ICON_SIZE_MAX, Math.max(ICON_SIZE_MIN, Math.round(size)))
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

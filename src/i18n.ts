import type { Lang } from './types'
import { en } from './locales/en'
import { zh } from './locales/zh'
import type { BaseKey } from './locales/zh'
import { errorMessage } from './locales/errors'
import type { ErrorCode } from './locales/errors'

/** Static UI keys plus one `error.<CODE>` key per backend error code. */
export type Key = BaseKey | `error.${ErrorCode}`


const dictionaries: Record<Lang, Record<BaseKey, string>> = { 'zh-CN': zh, en }

/**
 * Language the UI is currently painted in. The crash screen renders outside the app's
 * state (and its boundary has no props), so it reads the language from here instead of
 * falling back to Chinese.
 */
let currentLang: Lang = 'zh-CN'

export const setUiLanguage = (lang: Lang) => {
  currentLang = lang
}

export const uiLanguage = (): Lang => currentLang

export function translate(lang: Lang, key: Key, vars?: Record<string, string | number>): string {
  const template = key.startsWith('error.')
    ? errorMessage(lang, key.slice('error.'.length))
    : dictionaries[lang][key as BaseKey] ?? dictionaries['zh-CN'][key as BaseKey] ?? key
  if (!vars) return template
  return template.replace(/\{(\w+)\}/g, (_, name: string) => String(vars[name] ?? ''))
}

/**
 * Key for a backend `ApiError.code`. Codes without a table entry still render a readable
 * sentence, so this is safe for values added by a newer backend.
 */
export const errorKey = (code: string): Key => `error.${code}` as Key

/** Signature of the translator bound to one language. */
export type Translate = (key: Key, vars?: Record<string, string | number>) => string

export function makeT(lang: Lang): Translate {
  return (key, vars) => translate(lang, key, vars)
}

/** Localized drive-type caption ("本地磁盘"); empty for kinds the sidebar also skips. */
export function driveKindLabel(t: Translate, kind: string): string {
  switch (kind) {
    case 'fixed':
      return t('drive.fixed')
    case 'removable':
      return t('drive.removable')
    case 'network':
      return t('drive.network')
    case 'cdrom':
      return t('drive.cdrom')
    case 'ramdisk':
      return t('drive.ramdisk')
    case 'location':
      return t('drive.location')
    default:
      return ''
  }
}

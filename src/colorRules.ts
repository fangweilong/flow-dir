import type { CSSProperties } from 'react'
import type { ColorRule } from './persist'

/** Escapes a glob so only `*` and `?` stay special. */
const globToRegExp = (pattern: string) => {
  let source = '^'
  for (const ch of pattern) {
    if (ch === '*') source += '.*'
    else if (ch === '?') source += '.'
    else source += ch.replace(/[.+^${}()|[\]\\]/g, '\\$&')
  }
  return new RegExp(`${source}$`, 'i')
}

/**
 * Rules use a case-insensitive glob when they contain `*`/`?`; a plain string matches
 * anywhere in the name, which is the convenient case for "backup" style markers.
 */
const matches = (pattern: string, name: string) => {
  const trimmed = pattern.trim()
  if (!trimmed) return false
  if (!trimmed.includes('*') && !trimmed.includes('?')) {
    return name.toLowerCase().includes(trimmed.toLowerCase())
  }
  return globToRegExp(trimmed).test(name)
}

/** First matching rule wins, so the list order doubles as priority. */
export const colorRuleFor = (
  name: string,
  isDir: boolean,
  rules: ColorRule[] | undefined,
): ColorRule | undefined => {
  if (!rules?.length) return undefined
  for (const rule of rules) {
    if (isDir && !rule.folders) continue
    if (matches(rule.pattern, name)) return rule
  }
  return undefined
}

export const colorRuleStyle = (rule: ColorRule | undefined): CSSProperties => {
  if (!rule) return {}
  const style: CSSProperties = {}
  if (rule.foreground) style.color = rule.foreground
  if (rule.background) {
    style.backgroundColor = rule.background
    // A light highlight needs dark text even in a dark theme.
    if (!rule.foreground) style.color = '#1e293b'
  }
  return style
}

import type { SearchFilter } from '../../api'
import type { SearchItem } from '../../types'

export type OpenTab = { id: string; title: string; path: string }
export type SearchScope = 'folder' | 'drive' | 'all-tabs' | `tab:${string}`
export type SearchSort = { key: 'name' | 'size' | 'mtime' | 'path' | 'type'; dir: 'asc' | 'desc' }

/** Filter values live in localStorage so a restart keeps the last search setup. */
export type UiFilter = {
  minMb: string
  maxMb: string
  days: string
  exts: string
  kind: '' | 'file' | 'dir'
}

export const HISTORY_KEY = 'flowdir.search.history'
export const FILTER_KEY = 'flowdir.search.filters'
export const LIMIT_KEY = 'flowdir.search.limit'
export const HISTORY_MAX = 20
export const LIMITS = [500, 1000, 2000, 5000]

export const EMPTY_FILTER: UiFilter = { minMb: '', maxMb: '', days: '', exts: '', kind: '' }

export function stored<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key)
    return raw ? (JSON.parse(raw) as T) : fallback
  } catch {
    return fallback
  }
}

export function store(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value))
  } catch {
    // Private/sandboxed webviews may refuse storage; the search still works.
  }
}

/** Fallback order while the backend answers, and if the call fails. */
export function sortResults(items: SearchItem[], sort: SearchSort, collator: Intl.Collator): SearchItem[] {
  const factor = sort.dir === 'desc' ? -1 : 1
  return [...items].sort((a, b) => {
    if (a.isDir !== b.isDir) return a.isDir ? -1 : 1
    const primary =
      sort.key === 'size'
        ? a.size - b.size
        : sort.key === 'mtime'
          ? a.mtime - b.mtime
          : sort.key === 'path'
            ? collator.compare(a.path, b.path)
            : sort.key === 'type'
              ? collator.compare(
                  a.name.lastIndexOf('.') > 0 ? a.name.slice(a.name.lastIndexOf('.') + 1).toLowerCase() : '',
                  b.name.lastIndexOf('.') > 0 ? b.name.slice(b.name.lastIndexOf('.') + 1).toLowerCase() : '',
                )
              : collator.compare(a.name, b.name)
    if (primary !== 0) return primary * factor
    return collator.compare(a.name, b.name)
  })
}

export function toFilter(ui: UiFilter): SearchFilter {
  const mb = (value: string) => {
    const parsed = Number(value)
    return Number.isFinite(parsed) && parsed > 0 ? Math.round(parsed * 1024 * 1024) : 0
  }
  const days = Number(ui.days)
  return {
    minSize: mb(ui.minMb),
    maxSize: mb(ui.maxMb),
    afterMs: Number.isFinite(days) && days > 0 ? Date.now() - days * 86_400_000 : 0,
    beforeMs: 0,
    extensions: ui.exts
      .split(/[\s,;]+/)
      .map((ext) => ext.trim().replace(/^\./, '').toLowerCase())
      .filter(Boolean),
    kind: ui.kind,
  }
}

export const filterActive = (filter: SearchFilter) =>
  !!(
    filter.minSize ||
    filter.maxSize ||
    filter.afterMs ||
    filter.beforeMs ||
    filter.extensions?.length ||
    filter.kind
  )

/** Marks literal query hits in a snippet; regex hits are left plain. */
export function Highlight({ text, needle }: { text: string; needle: string }) {
  if (!needle) return <>{text}</>
  const lower = text.toLowerCase()
  const target = needle.toLowerCase()
  if (!lower.includes(target)) return <>{text}</>
  const parts: { text: string; hit: boolean }[] = []
  let cursor = 0
  while (cursor < text.length) {
    const at = lower.indexOf(target, cursor)
    if (at < 0) {
      parts.push({ text: text.slice(cursor), hit: false })
      break
    }
    if (at > cursor) parts.push({ text: text.slice(cursor, at), hit: false })
    parts.push({ text: text.slice(at, at + needle.length), hit: true })
    cursor = at + needle.length
  }
  return (
    <>
      {parts.map((part, index) =>
        part.hit ? (
          <mark key={index} className="rounded bg-amber-300/40 text-fg-strong">
            {part.text}
          </mark>
        ) : (
          <span key={index}>{part.text}</span>
        ),
      )}
    </>
  )
}

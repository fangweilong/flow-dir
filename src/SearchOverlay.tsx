import { useEffect, useMemo, useRef, useState } from 'react'
import { searchCancel, searchStart, sortOrder } from './api'
import { makeT } from './i18n'
import { formatSize, parentPath, volumeRoot } from './paths'
import { FileIcon } from './icons'
import { BASE_FONT_SIZE, clampFontSize } from './fonts'
import type { ApiError, Lang } from './types'
import type { SearchItem } from './types'

type SearchScope = 'folder' | 'drive'
type SearchSort = { key: 'name' | 'size' | 'mtime'; dir: 'asc' | 'desc' }

/** Fallback order while the backend answers, and if the call fails. */
function sortResults(items: SearchItem[], sort: SearchSort, collator: Intl.Collator): SearchItem[] {
  const factor = sort.dir === 'desc' ? -1 : 1
  return [...items].sort((a, b) => {
    if (a.isDir !== b.isDir) return a.isDir ? -1 : 1
    const primary =
      sort.key === 'size'
        ? a.size - b.size
        : sort.key === 'mtime'
          ? a.mtime - b.mtime
          : collator.compare(a.name, b.name)
    if (primary !== 0) return primary * factor
    return collator.compare(a.name, b.name)
  })
}

type Props = {
  root: string
  lang: Lang
  /** Base UI text size; icons request their raster at the same scale. */
  fontSize: number
  preferEverything: boolean
  /** Built-in scan follows the pane's show-hidden setting. */
  showHidden: boolean
  onClose: () => void
  /** Opens the hit itself: a folder navigates, a file goes to its default program. */
  onActivate: (item: SearchItem) => void
  /** Reveals the containing folder (Alt+Enter). */
  onOpen: (folder: string) => void
}

export function SearchOverlay({
  root,
  lang,
  fontSize,
  preferEverything,
  showHidden,
  onClose,
  onActivate,
  onOpen,
}: Props) {
  const t = makeT(lang)
  const iconScale = clampFontSize(fontSize) / BASE_FONT_SIZE
  const [query, setQuery] = useState('')
  const [items, setItems] = useState<SearchItem[]>([])
  const [provider, setProvider] = useState('')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<ApiError | null>(null)
  const [activePath, setActivePath] = useState<string | null>(null)
  const [scope, setScope] = useState<SearchScope>('folder')
  const [sort, setSort] = useState<SearchSort>({ key: 'name', dir: 'asc' })
  const gen = useRef(0)
  const inputRef = useRef<HTMLInputElement>(null)

  const driveRoot = volumeRoot(root)
  const searchRoot = scope === 'drive' && driveRoot ? driveRoot : root
  const collator = useMemo(
    () => new Intl.Collator(lang === 'zh-CN' ? 'zh-CN' : 'en', { numeric: true, sensitivity: 'base' }),
    [lang],
  )
  /**
   * Rows the backend ordered with Explorer's comparator, so a hit list and a file list
   * show the same names in the same order. While the call is in flight (and if it fails)
   * the local collator keeps the list usable.
   */
  const [permutation, setPermutation] = useState<number[] | null>(null)
  useEffect(() => {
    if (!items.length) {
      setPermutation(null)
      return
    }
    let stale = false
    const timer = window.setTimeout(() => {
      void sortOrder(
        items.map((item) => ({ name: item.name, size: item.size, mtime: item.mtime, isDir: item.isDir })),
        sort.key,
        sort.dir,
      )
        .then((order) => {
          if (!stale) setPermutation(order)
        })
        .catch(() => {
          if (!stale) setPermutation(null)
        })
    }, 120)
    return () => {
      stale = true
      window.clearTimeout(timer)
    }
  }, [items, sort])
  const ordered = useMemo(() => {
    if (permutation && permutation.length === items.length) {
      return permutation.map((index) => items[index]).filter(Boolean)
    }
    return sortResults(items, sort, collator)
  }, [items, sort, collator, permutation])
  const activeIndex = Math.max(0, ordered.findIndex((item) => item.path === activePath))
  const current = ordered[activeIndex]

  const move = (step: number) => {
    if (!ordered.length) return
    const base = activePath === null ? (step > 0 ? -1 : 0) : activeIndex
    const next = Math.max(0, Math.min(ordered.length - 1, base + step))
    setActivePath(ordered[next].path)
  }

  useEffect(() => {
    inputRef.current?.focus()
  }, [])

  useEffect(() => {
    const value = query.trim()
    if (!value) {
      setItems([])
      setActivePath(null)
      setProvider('')
      setLoading(false)
      setError(null)
      void searchCancel().catch(() => undefined)
      return
    }
    const id = ++gen.current
    // Results belong to the previous query until this one produces its own; keeping them
    // on screen would let Enter open a hit the user is no longer searching for.
    setItems([])
    setActivePath(null)
    setProvider('')
    setLoading(true)
    setError(null)
    void searchCancel().catch(() => undefined)
    const timer = window.setTimeout(() => {
      const acc: SearchItem[] = []
      searchStart(searchRoot, value, id, 500, preferEverything, showHidden, (batch) => {
        if (batch.gen !== gen.current) return
        if (batch.error) {
          setError(batch.error)
          setLoading(false)
          return
        }
        if (batch.items.length) {
          acc.push(...batch.items)
          setItems(acc.slice())
        }
        setProvider(batch.provider)
        if (batch.done) setLoading(false)
      }).catch((e: unknown) => {
        if (gen.current !== id) return
        setError({ code: 'SEARCH', detail: String(e) })
        setLoading(false)
      })
    }, 200)

    return () => window.clearTimeout(timer)
  }, [query, searchRoot, scope, preferEverything, showHidden])

  useEffect(() => {
    return () => {
      void searchCancel().catch(() => undefined)
    }
  }, [])

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-slate-950/30 pt-[12vh] backdrop-blur-[1px]"
      onMouseDown={(ev) => {
        if (ev.target === ev.currentTarget) onClose()
      }}
    >
      <div className="flex max-h-[64vh] w-[40rem] max-w-[calc(100vw-1.5rem)] flex-col overflow-hidden rounded-xl border border-line bg-surface shadow-2xl">
        <div className="flex items-center gap-2 border-b border-line px-3 py-2">
          <svg viewBox="0 0 16 16" className="h-4 w-4 shrink-0 text-muted-2" fill="none" stroke="currentColor" strokeWidth="1.5">
            <circle cx="7" cy="7" r="4.2" />
            <path d="m10.2 10.2 3 3" strokeLinecap="round" />
          </svg>
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Escape') onClose()
              if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
                e.preventDefault()
                if (!ordered.length) return
                move(e.key === 'ArrowDown' ? 1 : -1)
              }
              if (e.key === 'Enter' && current) {
                // Alt+Enter is the "open containing folder" escape hatch; plain Enter
                // opens the hit itself, matching Explorer's search results.
                if (e.altKey) onOpen(parentPath(current.path))
                else onActivate(current)
                onClose()
              }
            }}
            placeholder={t('search.placeholder')}
            className="h-7 min-w-0 flex-1 bg-transparent text-[0.8125rem] outline-none placeholder:text-muted-2"
          />
        </div>

        <div className="flex items-center gap-1.5 border-b border-line px-3 py-1.5 text-[0.65625rem] text-muted-2">
          <span>{t('search.scope')}</span>
          <select
            value={scope}
            onChange={(e) => setScope(e.target.value as SearchScope)}
            aria-label={t('search.scope')}
            className="h-6 shrink-0 rounded border border-line bg-surface-2 px-1 text-[0.65625rem] text-fg outline-none"
          >
            <option value="folder">{t('search.scopeFolder')}</option>
            {driveRoot && (
              <option value="drive">
                {t('search.scopeDrive', { name: driveRoot.replace(/\\$/, '') })}
              </option>
            )}
          </select>
          <span className="ml-2">{t('search.sort')}</span>
          <select
            value={sort.key}
            onChange={(e) => setSort((prev) => ({ ...prev, key: e.target.value as SearchSort['key'] }))}
            aria-label={t('search.sort')}
            className="h-6 shrink-0 rounded border border-line bg-surface-2 px-1 text-[0.65625rem] text-fg outline-none"
          >
            <option value="name">{t('search.sortName')}</option>
            <option value="size">{t('search.sortSize')}</option>
            <option value="mtime">{t('search.sortTime')}</option>
          </select>
          <button
            type="button"
            onClick={() => setSort((prev) => ({ ...prev, dir: prev.dir === 'asc' ? 'desc' : 'asc' }))}
            title={t(sort.dir === 'asc' ? 'search.sortAsc' : 'search.sortDesc')}
            aria-label={t(sort.dir === 'asc' ? 'search.sortAsc' : 'search.sortDesc')}
            className="grid h-6 w-5 shrink-0 place-items-center rounded border border-line bg-surface-2 text-fg hover:bg-surface-3"
          >
            {sort.dir === 'asc' ? '▲' : '▼'}
          </button>
          {provider && (
            <span className="ml-auto shrink-0 rounded bg-surface-3 px-1.5 py-0.5 text-[0.625rem] text-muted">
              {provider === 'everything' ? t('search.everything') : t('search.jwalk')}
            </span>
          )}
        </div>

        <div className="min-h-0 flex-1 overflow-auto">
          {loading && !items.length && (
            <div className="px-3 py-6 text-center text-[0.71875rem] text-muted-2">{t('search.searching')}</div>
          )}
          {!loading && !items.length && query.trim() && !error && (
            <div className="px-3 py-6 text-center text-[0.71875rem] text-muted-2">{t('search.empty')}</div>
          )}
          {error && (
            <div className="m-3 rounded border border-red-300 bg-red-50 p-2 text-[0.6875rem] text-red-800 dark:border-red-900 dark:bg-red-950/40 dark:text-red-200">
              {error.code} {error.detail}
            </div>
          )}

          {ordered.map((item, index) => (
            <button
              key={`${item.path}-${index}`}
              type="button"
              data-result={index}
              onMouseEnter={() => setActivePath(item.path)}
              onClick={() => {
                onActivate(item)
                onClose()
              }}
              className={`flex w-full items-center gap-2 border-b border-row-line px-3 py-1.5 text-left ${
                index === activeIndex ? 'bg-sel' : 'hover:bg-surface-3'
              }`}
            >
              <FileIcon name={item.name} dir={item.isDir} path={item.path} scale={iconScale} />
              <span className="min-w-0 flex-1 truncate text-[0.75rem] text-fg-strong">{item.name}</span>
              <span className="max-w-[58%] truncate font-mono text-[0.625rem] text-muted-2" dir="rtl">
                {parentPath(item.path)}
              </span>
              <span className="w-14 shrink-0 text-right text-[0.625rem] text-muted-2">
                {formatSize(item.size, item.isDir)}
              </span>
            </button>
          ))}
        </div>

        <div className="flex h-7 items-center justify-between border-t border-line px-3 text-[0.65625rem] text-muted-2">
          <span>{t('search.results', { count: ordered.length })}</span>
          <span>{t('search.hint')}</span>
        </div>
      </div>
    </div>
  )
}

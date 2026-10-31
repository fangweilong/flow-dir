import { useEffect, useMemo, useRef, useState } from 'react'
import { revealPath, searchCancel, searchStart, sortOrder } from './api'
import type { ContentOptions } from './api'
import { makeT } from './i18n'
import { formatSize, parentPath, volumeRoot } from './paths'
import { FileIcon } from './icons'
import { BASE_FONT_SIZE, clampFontSize, clampIconSize } from './fonts'
import type { ApiError, Lang, SearchItem } from './types'
import {
  type OpenTab,
  type SearchScope,
  type SearchSort,
  type UiFilter,
  EMPTY_FILTER,
  FILTER_KEY,
  HISTORY_KEY,
  HISTORY_MAX,
  Highlight,
  LIMIT_KEY,
  LIMITS,
  filterActive,
  sortResults,
  store,
  stored,
  toFilter,
} from './components/search/SearchHelpers'

export type { OpenTab }

type Props = {
  root: string
  openTabs?: OpenTab[]
  lang: Lang
  fontSize: number
  iconSize?: number
  preferEverything: boolean
  /** Built-in scan follows the pane's show-hidden setting. */
  showHidden: boolean
  onClose: () => void
  /** Opens the hit itself: a folder navigates, a file goes to its default program. */
  onActivate: (item: SearchItem) => void
  /** Reveals the containing folder (Alt+Enter). */
  onOpen: (folder: string) => void
  /** Recycle-bin delete with the app's undo stack behind it. */
  onDelete: (paths: string[]) => void
}

export function SearchOverlay({
  root,
  openTabs,
  lang,
  fontSize,
  iconSize,
  preferEverything,
  showHidden,
  onClose,
  onActivate,
  onOpen,
  onDelete,
}: Props) {
  const t = makeT(lang)
  const iconScale = clampFontSize(fontSize) / BASE_FONT_SIZE
  const iconPx = iconSize ? clampIconSize(iconSize) : undefined
  const [query, setQuery] = useState('')
  const [items, setItems] = useState<SearchItem[]>([])
  const [provider, setProvider] = useState('')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<ApiError | null>(null)
  const [activePath, setActivePath] = useState<string | null>(null)
  const [scope, setScope] = useState<SearchScope>('folder')
  const [sort, setSort] = useState<SearchSort>({ key: 'name', dir: 'asc' })
  const [limit, setLimit] = useState<number>(() => {
    const saved = stored(LIMIT_KEY, 500)
    return LIMITS.includes(saved) ? saved : 500
  })
  const [uiFilter, setUiFilter] = useState<UiFilter>(() => ({
    ...EMPTY_FILTER,
    ...(stored<Partial<UiFilter>>(FILTER_KEY, EMPTY_FILTER) ?? {}),
  }))
  const [showFilters, setShowFilters] = useState(false)
  const [contentOn, setContentOn] = useState(false)
  const [regexOn, setRegexOn] = useState(false)
  const [history, setHistory] = useState<string[]>(() => {
    const saved = stored<string[]>(HISTORY_KEY, [])
    return Array.isArray(saved) ? saved.filter((entry) => typeof entry === 'string') : []
  })
  const [historyOpen, setHistoryOpen] = useState(false)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [scrollTop, setScrollTop] = useState(0)
  const [viewHeight, setViewHeight] = useState(360)
  const gen = useRef(0)
  const inputRef = useRef<HTMLInputElement>(null)
  const scrollRef = useRef<HTMLDivElement>(null)

  const driveRoot = volumeRoot(root)
  const otherTabs = useMemo(
    () => openTabs?.filter((tab) => tab.path && tab.path.toLowerCase() !== root.toLowerCase()) ?? [],
    [openTabs, root],
  )
  const searchRoots = useMemo(() => {
    if (scope === 'drive' && driveRoot) return [driveRoot]
    if (scope === 'all-tabs' && openTabs?.length) {
      return Array.from(new Set(openTabs.map((tab) => tab.path).filter(Boolean)))
    }
    if (scope.startsWith('tab:')) {
      const tabPath = scope.slice(4)
      if (tabPath) return [tabPath]
    }
    return [root]
  }, [scope, driveRoot, openTabs, root])
  const searchRoot = searchRoots[0] || root
  const collator = useMemo(
    () => new Intl.Collator(lang === 'zh-CN' ? 'zh-CN' : 'en', { numeric: true, sensitivity: 'base' }),
    [lang],
  )
  const filter = useMemo(() => toFilter(uiFilter), [uiFilter])
  const content: ContentOptions = useMemo(
    () => ({ enabled: contentOn, regex: regexOn, maxBytes: 10 * 1024 * 1024 }),
    [contentOn, regexOn],
  )
  /**
   * Rows the backend ordered with Explorer's comparator, so a hit list and a file list
   * show the same names in the same order. While the call is in flight (and if it fails)
   * the local collator keeps the list usable.
   */
  const [permutation, setPermutation] = useState<number[] | null>(null)
  useEffect(() => {
    if (!items.length || loading) {
      setPermutation(null)
      return
    }
    let stale = false
    const timer = window.setTimeout(() => {
      void sortOrder(
        items.map((item) => ({ name: item.name, path: item.path, size: item.size, mtime: item.mtime, isDir: item.isDir })),
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
  }, [items, sort, loading])
  const ordered = useMemo(() => {
    if (permutation && permutation.length === items.length) {
      return permutation.map((index) => items[index]).filter(Boolean)
    }
    return sortResults(items, sort, collator)
  }, [items, sort, collator, permutation])
  const activeIndex = Math.max(0, ordered.findIndex((item) => item.path === activePath))
  const current = ordered[activeIndex]

  useEffect(() => store(FILTER_KEY, uiFilter), [uiFilter])
  useEffect(() => store(LIMIT_KEY, limit), [limit])

  const remember = (value: string) => {
    const trimmed = value.trim()
    if (!trimmed) return
    setHistory((prev) => {
      const next = [trimmed, ...prev.filter((entry) => entry !== trimmed)].slice(0, HISTORY_MAX)
      store(HISTORY_KEY, next)
      return next
    })
  }

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
      // Bump the generation before cancelling: a batch already in flight must not write
      // its hits back, or Enter would open a result for a query that was cleared.
      gen.current += 1
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
    setSelected(new Set())
    setActivePath(null)
    setProvider('')
    setLoading(true)
    setError(null)
    void searchCancel().catch(() => undefined)
    const timer = window.setTimeout(() => {
      const acc: SearchItem[] = []
      searchStart(
        searchRoot,
        value,
        id,
        limit,
        preferEverything,
        showHidden,
        filter,
        content,
        (batch) => {
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
        },
        searchRoots,
      ).catch((e: unknown) => {
        if (gen.current !== id) return
        setError({ code: 'SEARCH', detail: String(e) })
        setLoading(false)
      })
    }, 200)

    return () => window.clearTimeout(timer)
  }, [query, searchRoot, searchRoots, scope, preferEverything, showHidden, filter, content, limit])

  useEffect(() => {
    return () => {
      void searchCancel().catch(() => undefined)
    }
  }, [])

  // Windowing keeps 5000 hits responsive; rows are fixed-height so the slice is exact.
  // Clamping the first row keeps a shrunken result set from rendering a blank page when
  // the scroll offset is still where the previous, longer list left it.
  const rowHeight = contentOn ? 54 : 32
  useEffect(() => {
    const el = scrollRef.current
    if (!el) return
    const measure = () => setViewHeight(el.clientHeight)
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(el)
    return () => observer.disconnect()
  }, [])
  const rowsPerView = Math.ceil(viewHeight / rowHeight)
  const maxStart = Math.max(0, ordered.length - rowsPerView)
  const start = Math.min(maxStart, Math.max(0, Math.floor(scrollTop / rowHeight) - 6))
  const end = Math.min(ordered.length, Math.max(start + 12, Math.ceil((scrollTop + viewHeight) / rowHeight) + 6))
  const visible = ordered.slice(start, end)

  useEffect(() => {
    const el = scrollRef.current
    if (!el || !activePath) return
    const index = ordered.findIndex((item) => item.path === activePath)
    if (index < 0) return
    const top = index * rowHeight
    if (top < el.scrollTop) el.scrollTop = top
    else if (top + rowHeight > el.scrollTop + el.clientHeight) el.scrollTop = top + rowHeight - el.clientHeight
  }, [activePath, ordered, rowHeight])

  const toggleSelected = (path: string) => {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(path)) next.delete(path)
      else next.add(path)
      return next
    })
  }

  const selectedPaths = useMemo(() => ordered.filter((item) => selected.has(item.path)).map((item) => item.path), [ordered, selected])

  const revealSelected = () => {
    for (const path of selectedPaths.slice(0, 50)) void revealPath(path).catch(() => undefined)
  }

  const copySelected = () => {
    if (!selectedPaths.length) return
    void navigator.clipboard.writeText(selectedPaths.join('\r\n')).catch(() => undefined)
  }

  const deleteSelected = () => {
    if (!selectedPaths.length) return
    if (!window.confirm(t('search.deleteConfirm', { count: selectedPaths.length }))) return
    onDelete(selectedPaths)
    setSelected(new Set())
  }

  const chips: { label: string; clear: () => void }[] = []
  if (filter.minSize) chips.push({ label: `≥ ${formatSize(filter.minSize, false)}`, clear: () => setUiFilter((f) => ({ ...f, minMb: '' })) })
  if (filter.maxSize) chips.push({ label: `≤ ${formatSize(filter.maxSize, false)}`, clear: () => setUiFilter((f) => ({ ...f, maxMb: '' })) })
  if (uiFilter.days) chips.push({ label: t('search.lastDays', { days: uiFilter.days }), clear: () => setUiFilter((f) => ({ ...f, days: '' })) })
  if (filter.extensions?.length) chips.push({ label: filter.extensions.map((e) => `.${e}`).join(' '), clear: () => setUiFilter((f) => ({ ...f, exts: '' })) })
  if (filter.kind) chips.push({ label: filter.kind === 'dir' ? t('search.kindDir') : t('search.kindFile'), clear: () => setUiFilter((f) => ({ ...f, kind: '' })) })
  if (contentOn) chips.push({ label: t('search.content'), clear: () => setContentOn(false) })
  if (regexOn) chips.push({ label: t('search.regex'), clear: () => setRegexOn(false) })

  const inputClass =
    'h-6 shrink-0 rounded border border-line bg-surface-2 px-1 text-[0.65625rem] text-fg outline-none'

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-slate-950/30 pt-[10vh] backdrop-blur-[1px]"
      onMouseDown={(ev) => {
        if (ev.target === ev.currentTarget) onClose()
      }}
      // Escape closes from anywhere in the panel, not only while the query field has
      // focus; the scope and sort controls would otherwise trap the user.
      onKeyDown={(ev) => {
        if (ev.key === 'Escape' && !ev.nativeEvent.isComposing) {
          ev.preventDefault()
          onClose()
        }
      }}
    >
      <div className="flex max-h-[74vh] w-[46rem] max-w-[calc(100vw-1.5rem)] flex-col overflow-hidden rounded-xl border border-line bg-surface shadow-2xl">
        <div className="relative flex items-center gap-2 border-b border-line px-3 py-2">
          <svg viewBox="0 0 16 16" className="h-4 w-4 shrink-0 text-muted-2" fill="none" stroke="currentColor" strokeWidth="1.5">
            <circle cx="7" cy="7" r="4.2" />
            <path d="m10.2 10.2 3 3" strokeLinecap="round" />
          </svg>
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => {
              setQuery(e.target.value)
              setHistoryOpen(false)
            }}
            onFocus={() => setHistoryOpen(!query.trim() && history.length > 0)}
            onBlur={() => window.setTimeout(() => setHistoryOpen(false), 120)}
            onKeyDown={(e) => {
              // While an IME composes, Enter confirms a candidate and the arrows page
              // through it; moving the result selection instead would fight the input.
              if (e.nativeEvent.isComposing) return
              if (historyOpen && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) {
                e.preventDefault()
                const index = history.indexOf(query)
                const next =
                  e.key === 'ArrowDown'
                    ? Math.min(history.length - 1, index + 1)
                    : index <= 0
                      ? 0
                      : index - 1
                setQuery(history[next])
                return
              }
              if (!query.trim() && (e.key === 'ArrowDown' || e.key === 'ArrowUp') && history.length) {
                e.preventDefault()
                setHistoryOpen(true)
                setQuery(history[0])
                return
              }
              if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
                e.preventDefault()
                if (!ordered.length) return
                move(e.key === 'ArrowDown' ? 1 : -1)
              }
              if (e.key === 'Enter' && current) {
                // Alt+Enter is the "open containing folder" escape hatch; plain Enter
                // opens the hit itself, matching Explorer's search results.
                remember(query)
                if (e.altKey) onOpen(parentPath(current.path))
                else onActivate(current)
                onClose()
              }
            }}
            placeholder={t('search.placeholder')}
            className="h-7 min-w-0 flex-1 bg-transparent text-[0.8125rem] outline-none placeholder:text-muted-2"
          />
          <button
            type="button"
            onClick={() => setShowFilters((open) => !open)}
            aria-expanded={showFilters}
            title={t('search.filters')}
            className={`shrink-0 rounded px-1.5 py-0.5 text-[0.65625rem] ${
              showFilters || filterActive(filter) ? 'bg-sel text-fg-strong' : 'text-muted hover:bg-surface-3'
            }`}
          >
            {t('search.filters')}
          </button>
          {historyOpen && (
            <div className="absolute left-8 right-3 top-full z-10 max-h-56 overflow-auto rounded-b-md border border-line bg-surface py-1 shadow-xl">
              <div className="flex items-center justify-between px-2 pb-1 text-[0.625rem] text-muted-2">
                <span>{t('search.history')}</span>
                <button
                  type="button"
                  onMouseDown={(ev) => ev.preventDefault()}
                  onClick={() => {
                    setHistory([])
                    store(HISTORY_KEY, [])
                    setHistoryOpen(false)
                  }}
                  className="hover:text-fg"
                >
                  {t('search.clearHistory')}
                </button>
              </div>
              {history.map((entry) => (
                <button
                  key={entry}
                  type="button"
                  onMouseDown={(ev) => ev.preventDefault()}
                  onClick={() => {
                    setQuery(entry)
                    setHistoryOpen(false)
                  }}
                  className="block w-full truncate px-2 py-1 text-left text-[0.71875rem] text-fg hover:bg-surface-3"
                >
                  {entry}
                </button>
              ))}
            </div>
          )}
        </div>

        {showFilters && (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 border-b border-line px-3 py-1.5 text-[0.65625rem] text-muted-2">
            <label className="flex items-center gap-1">
              {t('search.minSize')}
              <input
                value={uiFilter.minMb}
                onChange={(e) => setUiFilter((f) => ({ ...f, minMb: e.target.value }))}
                inputMode="decimal"
                className={`${inputClass} w-12 text-right`}
              />
              MB
            </label>
            <label className="flex items-center gap-1">
              {t('search.maxSize')}
              <input
                value={uiFilter.maxMb}
                onChange={(e) => setUiFilter((f) => ({ ...f, maxMb: e.target.value }))}
                inputMode="decimal"
                className={`${inputClass} w-12 text-right`}
              />
              MB
            </label>
            <label className="flex items-center gap-1">
              {t('search.modified')}
              <select
                value={uiFilter.days}
                onChange={(e) => setUiFilter((f) => ({ ...f, days: e.target.value }))}
                className={inputClass}
              >
                <option value="">{t('search.anyTime')}</option>
                <option value="1">{t('search.lastDays', { days: 1 })}</option>
                <option value="7">{t('search.lastDays', { days: 7 })}</option>
                <option value="30">{t('search.lastDays', { days: 30 })}</option>
                <option value="365">{t('search.lastDays', { days: 365 })}</option>
              </select>
            </label>
            <label className="flex items-center gap-1">
              {t('search.kind')}
              <select
                value={uiFilter.kind}
                onChange={(e) => setUiFilter((f) => ({ ...f, kind: e.target.value as UiFilter['kind'] }))}
                className={inputClass}
              >
                <option value="">{t('search.kindAny')}</option>
                <option value="file">{t('search.kindFile')}</option>
                <option value="dir">{t('search.kindDir')}</option>
              </select>
            </label>
            <label className="flex min-w-[8rem] flex-1 items-center gap-1">
              {t('search.exts')}
              <input
                value={uiFilter.exts}
                onChange={(e) => setUiFilter((f) => ({ ...f, exts: e.target.value }))}
                placeholder=".log .md"
                className={`${inputClass} min-w-0 flex-1`}
              />
            </label>
            <label className="flex items-center gap-1">
              <input type="checkbox" checked={contentOn} onChange={(e) => setContentOn(e.target.checked)} />
              {t('search.content')}
            </label>
            {contentOn && (
              <label className="flex items-center gap-1">
                <input type="checkbox" checked={regexOn} onChange={(e) => setRegexOn(e.target.checked)} />
                {t('search.regex')}
              </label>
            )}
            <button
              type="button"
              onClick={() => setUiFilter(EMPTY_FILTER)}
              className="ml-auto rounded border border-line px-1.5 py-0.5 hover:text-fg"
            >
              {t('search.clearFilters')}
            </button>
          </div>
        )}

        {chips.length > 0 && (
          <div className="flex flex-wrap items-center gap-1 border-b border-line px-3 py-1">
            {chips.map((chip, index) => (
              <button
                key={`${chip.label}-${index}`}
                type="button"
                onClick={chip.clear}
                title={t('search.removeFilter')}
                className="rounded-full border border-line bg-surface-2 px-1.5 py-0.5 text-[0.625rem] text-muted hover:text-fg"
              >
                {chip.label} ×
              </button>
            ))}
          </div>
        )}

        <div className="flex items-center gap-1.5 border-b border-line px-3 py-1.5 text-[0.65625rem] text-muted-2">
          <span>{t('search.scope')}</span>
          <select
            value={scope}
            onChange={(e) => setScope(e.target.value as SearchScope)}
            aria-label={t('search.scope')}
            className={inputClass}
          >
            <option value="folder">{t('search.scopeFolder')}</option>
            {driveRoot && (
              <option value="drive">
                {t('search.scopeDrive', { name: driveRoot.replace(/\\$/, '') })}
              </option>
            )}
            {openTabs && openTabs.length > 1 && (
              <option value="all-tabs">{t('search.scopeAllTabs')}</option>
            )}
            {otherTabs.map((tab) => (
              <option key={tab.id} value={`tab:${tab.path}`}>
                {t('search.scopeTab', { name: tab.title })}
              </option>
            ))}
          </select>
          <span className="ml-2">{t('search.sort')}</span>
          <select
            value={sort.key}
            onChange={(e) => setSort((prev) => ({ ...prev, key: e.target.value as SearchSort['key'] }))}
            aria-label={t('search.sort')}
            className={inputClass}
          >
            <option value="name">{t('search.sortName')}</option>
            <option value="size">{t('search.sortSize')}</option>
            <option value="mtime">{t('search.sortTime')}</option>
            <option value="path">{t('search.sortPath')}</option>
            <option value="type">{t('search.sortType')}</option>
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
          <span className="ml-2">{t('search.limit')}</span>
          <select
            value={limit}
            onChange={(e) => setLimit(Number(e.target.value))}
            aria-label={t('search.limit')}
            className={inputClass}
          >
            {LIMITS.map((value) => (
              <option key={value} value={value}>
                {value}
              </option>
            ))}
          </select>
          {provider && (
            <span className="ml-auto shrink-0 rounded bg-surface-3 px-1.5 py-0.5 text-[0.625rem] text-muted">
              {provider === 'everything' ? t('search.everything') : t('search.jwalk')}
            </span>
          )}
        </div>

        <div
          ref={scrollRef}
          className="min-h-0 flex-1 overflow-auto"
          onScroll={(ev) => setScrollTop(ev.currentTarget.scrollTop)}
        >
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

          {ordered.length > 0 && (
            <div style={{ height: start * rowHeight }} />
          )}
          {visible.map((item, offset) => {
            const index = start + offset
            return (
              <div
                key={`${item.path}-${index}`}
                onMouseEnter={() => setActivePath(item.path)}
                style={{ height: rowHeight }}
                className={`flex items-center gap-2 border-b border-row-line px-3 text-left ${
                  index === activeIndex ? 'bg-sel' : 'hover:bg-surface-3'
                }`}
              >
                <input
                  type="checkbox"
                  checked={selected.has(item.path)}
                  onChange={() => toggleSelected(item.path)}
                  aria-label={item.name}
                  className="shrink-0"
                />
                <button
                  type="button"
                  onClick={() => {
                    remember(query)
                    onActivate(item)
                    onClose()
                  }}
                  className="flex min-w-0 flex-1 items-center gap-2 text-left"
                >
                  <FileIcon name={item.name} dir={item.isDir} path={item.path} scale={iconScale} size={iconPx} />
                  <span className="flex min-w-0 flex-1 flex-col">
                    <span className="truncate text-[0.75rem] text-fg-strong">{item.name}</span>
                    {item.snippet && (
                      <span className="truncate text-[0.625rem] text-muted-2">
                        <Highlight text={item.snippet} needle={regexOn ? '' : query.trim()} />
                      </span>
                    )}
                  </span>
                  <span className="max-w-[48%] truncate font-mono text-[0.625rem] text-muted-2" dir="rtl">
                    {parentPath(item.path)}
                  </span>
                  <span className="w-14 shrink-0 text-right text-[0.625rem] text-muted-2">
                    {formatSize(item.size, item.isDir)}
                  </span>
                </button>
              </div>
            )
          })}
          {ordered.length > end && <div style={{ height: (ordered.length - end) * rowHeight }} />}
        </div>

        <div className="flex min-h-7 items-center justify-between gap-2 border-t border-line px-3 py-1 text-[0.65625rem] text-muted-2">
          <span className="shrink-0">{t('search.results', { count: ordered.length })}</span>
          <div className="flex min-w-0 items-center gap-1">
            {ordered.length > 0 && (
              <button
                type="button"
                onClick={() =>
                  setSelected((prev) =>
                    prev.size === ordered.length ? new Set() : new Set(ordered.map((item) => item.path)),
                  )
                }
                className="rounded border border-line px-1.5 py-0.5 hover:text-fg"
              >
                {selected.size === ordered.length ? t('search.clearSelection') : t('search.selectAll')}
              </button>
            )}
          {selectedPaths.length > 0 ? (
            <div className="flex min-w-0 items-center gap-1">
              <span className="shrink-0">{t('search.selected', { count: selectedPaths.length })}</span>
              <button type="button" onClick={revealSelected} className="rounded border border-line px-1.5 py-0.5 hover:text-fg">
                {t('search.reveal')}
              </button>
              <button type="button" onClick={copySelected} className="rounded border border-line px-1.5 py-0.5 hover:text-fg">
                {t('search.copyPaths')}
              </button>
              <button type="button" onClick={deleteSelected} className="rounded border border-line px-1.5 py-0.5 text-red-500 hover:bg-surface-3">
                {t('search.delete')}
              </button>
              <button type="button" onClick={() => setSelected(new Set())} className="rounded px-1.5 py-0.5 hover:text-fg">
                {t('search.clearSelection')}
              </button>
            </div>
          ) : (
            <span className="truncate">{t('search.hint')}</span>
          )}
          </div>
        </div>
      </div>
    </div>
  )
}
import { useEffect, useRef, useState } from 'react'
import { searchCancel, searchStart } from './api'
import { makeT } from './i18n'
import { formatSize, parentPath } from './paths'
import { FileIcon } from './icons'
import { BASE_FONT_SIZE, clampFontSize } from './fonts'
import type { ApiError, Lang } from './types'
import type { SearchItem } from './types'

type Props = {
  root: string
  lang: Lang
  /** Base UI text size; icons request their raster at the same scale. */
  fontSize: number
  preferEverything: boolean
  onClose: () => void
  onOpen: (folder: string) => void
}

export function SearchOverlay({ root, lang, fontSize, preferEverything, onClose, onOpen }: Props) {
  const t = makeT(lang)
  const iconScale = clampFontSize(fontSize) / BASE_FONT_SIZE
  const [query, setQuery] = useState('')
  const [items, setItems] = useState<SearchItem[]>([])
  const [provider, setProvider] = useState('')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<ApiError | null>(null)
  const [active, setActive] = useState(0)
  const gen = useRef(0)
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    inputRef.current?.focus()
  }, [])

  useEffect(() => {
    const value = query.trim()
    if (!value) {
      setItems([])
      setLoading(false)
      setError(null)
      void searchCancel().catch(() => undefined)
      return
    }
    const id = ++gen.current
    setLoading(true)
    setError(null)
    const timer = window.setTimeout(() => {
      const acc: SearchItem[] = []
      searchStart(root, value, id, 500, preferEverything, (batch) => {
        if (batch.gen !== gen.current) return
        if (batch.error) {
          setError(batch.error)
          setLoading(false)
          return
        }
        if (batch.items.length) {
          acc.push(...batch.items)
          setItems(acc.slice())
          setActive(0)
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
  }, [query, root, preferEverything])

  useEffect(() => {
    return () => {
      void searchCancel().catch(() => undefined)
    }
  }, [])

  const current = items[active]

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
                if (!items.length) return
                const step = e.key === 'ArrowDown' ? 1 : -1
                setActive((prev) => Math.max(0, Math.min(items.length - 1, prev + step)))
              }
              if (e.key === 'Enter' && current) {
                onOpen(parentPath(current.path))
                onClose()
              }
            }}
            placeholder={t('search.placeholder')}
            className="h-7 min-w-0 flex-1 bg-transparent text-[0.8125rem] outline-none placeholder:text-muted-2"
          />
          {provider && (
            <span className="shrink-0 rounded bg-surface-3 px-1.5 py-0.5 text-[0.625rem] text-muted">
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

          {items.map((item, index) => (
            <button
              key={`${item.path}-${index}`}
              type="button"
              data-result={index}
              onMouseEnter={() => setActive(index)}
              onClick={() => {
                onOpen(parentPath(item.path))
                onClose()
              }}
              className={`flex w-full items-center gap-2 border-b border-line/60 px-3 py-1.5 text-left ${
                index === active ? 'bg-sel' : 'hover:bg-surface-3'
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
          <span>{t('search.results', { count: items.length })}</span>
          <span>{t('search.hint')}</span>
        </div>
      </div>
    </div>
  )
}

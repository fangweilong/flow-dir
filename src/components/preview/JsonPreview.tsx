import { useMemo, useState } from 'react'
import type { Translate } from '../../i18n'

type Props = {
  text: string | null | undefined
  t: Translate
}

type Mode = 'tree' | 'format' | 'compact' | 'raw'

function highlightMatch(text: string, search: string): React.ReactNode {
  if (!search) return text
  const parts = text.split(new RegExp(`(${search.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')})`, 'gi'))
  if (parts.length === 1) return text
  return parts.map((part, i) =>
    part.toLowerCase() === search.toLowerCase() ? (
      <mark key={i} className="rounded bg-amber-300 dark:bg-amber-600 px-0.5 text-black">
        {part}
      </mark>
    ) : (
      part
    )
  )
}

function JsonTreeNode({
  nodeKey,
  value,
  path,
  depth,
  collapsedPaths,
  toggleCollapse,
  search,
}: {
  nodeKey?: string
  value: unknown
  path: string
  depth: number
  collapsedPaths: Set<string>
  toggleCollapse: (p: string) => void
  search: string
}) {
  const isObject = value !== null && typeof value === 'object'
  const isArray = Array.isArray(value)
  const isCollapsed = collapsedPaths.has(path)

  if (isObject) {
    const entries = isArray
      ? (value as unknown[]).map((v, i) => [String(i), v] as const)
      : Object.entries(value as Record<string, unknown>)
    const openBrace = isArray ? '[' : '{'
    const closeBrace = isArray ? ']' : '}'
    const count = entries.length

    return (
      <div className="font-mono text-[0.6875rem] leading-relaxed">
        <div
          className="flex items-center gap-1 cursor-pointer select-none rounded hover:bg-surface-3/60 px-1 py-0.5"
          onClick={() => toggleCollapse(path)}
          style={{ paddingLeft: `${depth * 1.25 + 0.25}rem` }}
        >
          <span className="text-[0.625rem] text-muted-2 w-3 text-center inline-block">
            {isCollapsed ? '▶' : '▼'}
          </span>
          {nodeKey !== undefined && (
            <span className="text-cyan-600 dark:text-cyan-400 font-medium">
              "{highlightMatch(nodeKey, search)}":
            </span>
          )}
          <span className="text-muted-2">{openBrace}</span>
          {isCollapsed && (
            <span className="rounded bg-surface-3 px-1.5 text-[0.625rem] text-muted">
              {count} {isArray ? 'items' : 'keys'}
            </span>
          )}
          {isCollapsed && <span className="text-muted-2">{closeBrace}</span>}
        </div>

        {!isCollapsed && (
          <div>
            {entries.map(([k, v]) => (
              <JsonTreeNode
                key={k}
                nodeKey={isArray ? undefined : k}
                value={v}
                path={`${path}.${k}`}
                depth={depth + 1}
                collapsedPaths={collapsedPaths}
                toggleCollapse={toggleCollapse}
                search={search}
              />
            ))}
            <div
              className="text-muted-2 px-1 py-0.5"
              style={{ paddingLeft: `${depth * 1.25 + 1.2}rem` }}
            >
              {closeBrace}
            </div>
          </div>
        )}
      </div>
    )
  }

  // Primitive value
  let formattedValue: React.ReactNode
  if (typeof value === 'string') {
    formattedValue = (
      <span className="text-emerald-600 dark:text-emerald-400">
        "{highlightMatch(value, search)}"
      </span>
    )
  } else if (typeof value === 'number') {
    formattedValue = (
      <span className="text-purple-600 dark:text-purple-400 font-semibold">
        {highlightMatch(String(value), search)}
      </span>
    )
  } else if (typeof value === 'boolean') {
    formattedValue = (
      <span className="text-amber-600 dark:text-amber-400 font-semibold">
        {String(value)}
      </span>
    )
  } else if (value === null) {
    formattedValue = <span className="text-muted italic">null</span>
  } else {
    formattedValue = <span className="text-fg">{String(value)}</span>
  }

  return (
    <div
      className="flex items-center gap-1 font-mono text-[0.6875rem] leading-relaxed px-1 py-0.5"
      style={{ paddingLeft: `${depth * 1.25 + 1.2}rem` }}
    >
      {nodeKey !== undefined && (
        <span className="text-cyan-600 dark:text-cyan-400 font-medium">
          "{highlightMatch(nodeKey, search)}":
        </span>
      )}
      {formattedValue}
    </div>
  )
}

export function JsonPreview({ text, t }: Props) {
  const [mode, setMode] = useState<Mode>('tree')
  const [search, setSearch] = useState('')
  const [collapsedPaths, setCollapsedPaths] = useState<Set<string>>(new Set())
  const [copied, setCopied] = useState(false)

  const parsed = useMemo(() => {
    if (!text) return null
    try {
      return JSON.parse(text)
    } catch {
      return null
    }
  }, [text])

  const formatted = useMemo(() => {
    if (parsed === null) return text ?? ''
    return JSON.stringify(parsed, null, 2)
  }, [parsed, text])

  const compact = useMemo(() => {
    if (parsed === null) return text ?? ''
    return JSON.stringify(parsed)
  }, [parsed, text])

  const matchCount = useMemo(() => {
    if (!search || !formatted) return 0
    const regex = new RegExp(search.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'gi')
    return (formatted.match(regex) || []).length
  }, [search, formatted])

  const toggleCollapse = (path: string) => {
    setCollapsedPaths((prev) => {
      const next = new Set(prev)
      if (next.has(path)) {
        next.delete(path)
      } else {
        next.add(path)
      }
      return next
    })
  }

  const collapseAll = () => {
    if (!parsed || typeof parsed !== 'object') return
    const paths = new Set<string>()
    const collect = (val: unknown, curr: string) => {
      if (val !== null && typeof val === 'object') {
        paths.add(curr)
        Object.entries(val as Record<string, unknown>).forEach(([k, v]) => {
          collect(v, `${curr}.${k}`)
        })
      }
    }
    collect(parsed, 'root')
    setCollapsedPaths(paths)
  }

  const expandAll = () => {
    setCollapsedPaths(new Set())
  }

  const copyCurrent = () => {
    const toCopy = mode === 'compact' ? compact : formatted
    void navigator.clipboard.writeText(toCopy)
    setCopied(true)
    setTimeout(() => setCopied(false), 1500)
  }

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center justify-between pb-1.5 gap-1 flex-wrap">
        <div className="flex items-center gap-1">
          <input
            type="text"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder={t('preview.search')}
            className="h-6 w-28 sm:w-36 rounded border border-line bg-surface-2 px-1.5 text-[0.625rem] text-fg outline-none placeholder:text-muted-2 focus:border-blue-400"
          />
          {search && (
            <span className="text-[0.5625rem] text-muted tabular">
              {t('preview.searchMatches', { count: matchCount })}
            </span>
          )}
        </div>

        <div className="flex items-center gap-0.5">
          <button
            type="button"
            onClick={copyCurrent}
            className="rounded px-1.5 py-0.5 text-[0.625rem] text-muted hover:bg-surface-3 hover:text-fg"
          >
            {copied ? t('preview.copied') : t('preview.copy')}
          </button>
          <div className="flex rounded bg-surface-2 p-0.5 text-[0.625rem]">
            {(['tree', 'format', 'compact', 'raw'] as Mode[]).map((m) => (
              <button
                key={m}
                type="button"
                onClick={() => setMode(m)}
                className={`rounded px-1.5 py-0.5 ${
                  mode === m
                    ? 'bg-sel font-medium text-fg-strong'
                    : 'text-muted hover:text-fg'
                }`}
              >
                {m === 'tree' && t('preview.tree')}
                {m === 'format' && t('preview.format')}
                {m === 'compact' && t('preview.compact')}
                {m === 'raw' && t('preview.htmlSource')}
              </button>
            ))}
          </div>
        </div>
      </div>

      {mode === 'tree' && (
        <div className="flex items-center justify-end gap-1 pb-1">
          <button
            type="button"
            onClick={expandAll}
            className="rounded px-1.5 py-0.5 text-[0.5625rem] text-muted hover:bg-surface-3 hover:text-fg"
          >
            {t('preview.expandAll')}
          </button>
          <button
            type="button"
            onClick={collapseAll}
            className="rounded px-1.5 py-0.5 text-[0.5625rem] text-muted hover:bg-surface-3 hover:text-fg"
          >
            {t('preview.collapseAll')}
          </button>
        </div>
      )}

      {mode === 'tree' && parsed !== null ? (
        <div className="min-h-0 flex-1 overflow-auto rounded border border-line bg-surface p-2">
          <JsonTreeNode
            value={parsed}
            path="root"
            depth={0}
            collapsedPaths={collapsedPaths}
            toggleCollapse={toggleCollapse}
            search={search}
          />
        </div>
      ) : mode === 'format' && parsed !== null ? (
        <pre
          data-selectable
          className="min-h-0 flex-1 overflow-auto whitespace-pre font-mono text-[0.6875rem] leading-relaxed text-fg rounded border border-line bg-surface p-2"
        >
          {highlightMatch(formatted, search)}
        </pre>
      ) : mode === 'compact' && parsed !== null ? (
        <pre
          data-selectable
          className="min-h-0 flex-1 overflow-auto whitespace-pre-wrap break-all font-mono text-[0.6875rem] leading-relaxed text-fg rounded border border-line bg-surface p-2"
        >
          {highlightMatch(compact, search)}
        </pre>
      ) : (
        <pre
          data-selectable
          className="min-h-0 flex-1 overflow-auto whitespace-pre-wrap break-words font-mono text-[0.6875rem] leading-relaxed text-fg rounded border border-line bg-surface p-2"
        >
          {highlightMatch(text ?? '', search)}
        </pre>
      )}
    </div>
  )
}

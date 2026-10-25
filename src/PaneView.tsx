import { useEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { FileIcon } from './icons'
import { crumbs, formatSize, formatTime, isDir, joinPath, parentPath } from './paths'
import { openPath } from './api'
import { EMPTY_LIST } from './types'
import type { Drive, Entry, Lang, ListData, SortKey, SortState } from './types'
import { makeT } from './i18n'
import type { DropMode, PaneState, TabState } from './workspace'

/** Drive label for the address bar ("C:", "D:"); UNC and relative paths fall back to a dash. */
const driveRoot = (path: string | undefined): string => {
  const match = path ? /^([A-Za-z]:)/.exec(path) : null
  return match ? match[1].toUpperCase() : '⋯'
}

const ROW_H = 28
const GRID_FULL = 'minmax(0,1fr) 96px 142px'
const GRID_NO_TIME = 'minmax(0,1fr) 84px'
const GRID_NAME = 'minmax(0,1fr)'

/** Screen/client coordinates for a context-menu request. */
export type MenuPoint = {
  clientX: number
  clientY: number
  screenX: number
  screenY: number
  shiftKey: boolean
}

const pointOf = (ev: { clientX: number; clientY: number; screenX: number; screenY: number; shiftKey: boolean }): MenuPoint => ({
  clientX: ev.clientX,
  clientY: ev.clientY,
  screenX: ev.screenX,
  screenY: ev.screenY,
  shiftKey: ev.shiftKey,
})

type Props = {
  pane: PaneState
  tabs: Record<string, TabState>
  data: ListData | undefined
  active: boolean
  lang: Lang
  dropHint: DropMode | null
  draggingTabId: string | null
  onFocusPane: () => void
  onNewTab: () => void
  onCloseTab: (tabId: string) => void
  onActivateTab: (tabId: string) => void
  onNavigate: (tabId: string, path: string) => void
  onTabDragStart: (tabId: string, ev: React.PointerEvent) => void
  onSortChange: (sort: SortState) => void
  onToggleHidden: () => void
  onClipboard: (cut: boolean) => void
  onRowContextMenu: (entry: Entry, point: MenuPoint) => void
  onBlankContextMenu: (point: MenuPoint) => void
  onTabContextMenu: (tabId: string, point: MenuPoint) => void
  drives: Drive[]
  onDriveMenu: (point: MenuPoint) => void
  onProperties: (names: string[]) => void
  onRename: (name: string) => void
  onDelete: (names: string[], permanent: boolean) => void
  onPaste: () => void
  onRefresh: () => void
  update: (tabId: string, fn: (d: ListData) => ListData) => void
}

const HINT_STYLE: Record<Exclude<DropMode, 'center'>, string> = {
  left: 'left-0 top-0 bottom-0 w-1/2',
  right: 'right-0 top-0 bottom-0 w-1/2',
  top: 'left-0 right-0 top-0 h-1/2',
  bottom: 'left-0 right-0 bottom-0 h-1/2',
}

export function PaneView({
  pane,
  tabs,
  data,
  active,
  lang,
  dropHint,
  draggingTabId,
  onFocusPane,
  onNewTab,
  onCloseTab,
  onActivateTab,
  onNavigate,
  onTabDragStart,
  onSortChange,
  onToggleHidden,
  onClipboard,
  onRowContextMenu,
  onBlankContextMenu,
  onTabContextMenu,
  drives,
  onDriveMenu,
  onProperties,
  onRename,
  onDelete,
  onPaste,
  onRefresh,
  update,
}: Props) {
  const t = makeT(lang)
  const list = data ?? EMPTY_LIST
  const tab = tabs[pane.activeTabId]
  const scrollRef = useRef<HTMLDivElement>(null)
  const sectionRef = useRef<HTMLElement>(null)
  const [width, setWidth] = useState(0)
  const [addressDraft, setAddressDraft] = useState<string | null>(null)
  const addressRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (addressDraft === null) return
    const input = addressRef.current
    if (!input) return
    input.focus()
    input.select()
  }, [addressDraft])

  useEffect(() => {
    const el = sectionRef.current
    if (!el) return
    const observer = new ResizeObserver((entries) => setWidth(entries[0].contentRect.width))
    observer.observe(el)
    return () => observer.disconnect()
  }, [])

  const showTime = width === 0 || width >= 380
  const showSize = width === 0 || width >= 250
  const showFilter = width === 0 || width >= 420
  const grid = showTime ? GRID_FULL : showSize ? GRID_NO_TIME : GRID_NAME

  const view = useMemo(() => {
    const q = list.filter.trim().toLowerCase()
    return q ? list.entries.filter((e) => e.name.toLowerCase().includes(q)) : list.entries
  }, [list.entries, list.filter])

  const virtualizer = useVirtualizer({
    count: view.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_H,
    overscan: 10,
  })

  const selectedSize = useMemo(
    () =>
      list.entries.reduce(
        (sum, e) => (list.selected.has(e.name) && !isDir(e.attrs) ? sum + e.size : sum),
        0,
      ),
    [list.entries, list.selected],
  )

  const activate = (entry: Entry) => {
    if (!tab) return
    const full = joinPath(tab.path, entry.name)
    if (isDir(entry.attrs)) onNavigate(tab.id, full)
    else void openPath(full).catch(() => undefined)
  }

  const changeSort = (key: SortKey) => {
    const dir = pane.sort.key === key && pane.sort.dir === 'asc' ? 'desc' : 'asc'
    onSortChange({ key, dir })
  }

  const selectRow = (index: number, ev: React.MouseEvent) => {
    const entry = view[index]
    if (!entry || !tab) return
    update(tab.id, (d) => {
      if (ev.shiftKey && d.anchor >= 0) {
        const [a, b] = [Math.min(d.anchor, index), Math.max(d.anchor, index)]
        return { ...d, selected: new Set(view.slice(a, b + 1).map((e) => e.name)) }
      }
      if (ev.ctrlKey || ev.metaKey) {
        const next = new Set(d.selected)
        if (next.has(entry.name)) next.delete(entry.name)
        else next.add(entry.name)
        return { ...d, selected: next, anchor: index }
      }
      return { ...d, selected: new Set([entry.name]), anchor: index }
    })
  }

  const onKeyDown = (ev: React.KeyboardEvent) => {
    if (!tab) return
    if (ev.altKey && ev.key === 'Enter') {
      ev.preventDefault()
      const names = list.selected.size
        ? Array.from(list.selected)
        : view[list.anchor]
          ? [view[list.anchor].name]
          : []
      if (names.length) onProperties(names)
      return
    }
    if (ev.altKey && ev.key === 'ArrowLeft') {
      ev.preventDefault()
      if (tab.histIdx > 0) onNavigate(tab.id, tab.history[tab.histIdx - 1])
      return
    }
    if (ev.altKey && ev.key === 'ArrowRight') {
      ev.preventDefault()
      if (tab.histIdx < tab.history.length - 1) onNavigate(tab.id, tab.history[tab.histIdx + 1])
      return
    }
    if (ev.altKey && ev.key === 'ArrowUp') {
      ev.preventDefault()
      onNavigate(tab.id, parentPath(tab.path))
      return
    }
    if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === 'l') {
      ev.preventDefault()
      setAddressDraft(tab.path)
      return
    }
    if ((ev.ctrlKey || ev.metaKey) && (ev.key === 'c' || ev.key === 'x')) {
      ev.preventDefault()
      onClipboard(ev.key === 'x')
      return
    }
    if ((ev.ctrlKey || ev.metaKey) && ev.key === 'a') {
      ev.preventDefault()
      update(tab.id, (d) => ({ ...d, selected: new Set(view.map((e) => e.name)) }))
      return
    }
    if (ev.key === 'ArrowDown' || ev.key === 'ArrowUp') {
      ev.preventDefault()
      if (!view.length) return
      const step = ev.key === 'ArrowDown' ? 1 : -1
      const current = list.anchor < 0 ? (step > 0 ? -1 : 0) : list.anchor
      const next = Math.max(0, Math.min(view.length - 1, current + step))
      const base = list.anchor < 0 ? next : list.anchor
      update(tab.id, (d) => {
        if (!ev.shiftKey) return { ...d, anchor: next, selected: new Set([view[next].name]) }
        const [a, b] = [Math.min(base, next), Math.max(base, next)]
        return { ...d, anchor: next, selected: new Set(view.slice(a, b + 1).map((e) => e.name)) }
      })
      virtualizer.scrollToIndex(next, { align: 'auto' })
      return
    }
    if (ev.key === 'Enter') {
      const entry = view[list.anchor]
      if (entry) activate(entry)
      return
    }
    if (ev.key === 'Backspace') {
      ev.preventDefault()
      onNavigate(tab.id, parentPath(tab.path))
      return
    }
    if (ev.key === 'Escape') {
      update(tab.id, (d) => ({ ...d, selected: new Set(), anchor: -1 }))
    }
    if (ev.key === 'F2') {
      ev.preventDefault()
      const target = list.selected.size === 1 ? Array.from(list.selected)[0] : view[list.anchor]?.name
      if (target) onRename(target)
      return
    }
    if (ev.key === 'Delete') {
      if (!list.selected.size) return
      ev.preventDefault()
      onDelete(Array.from(list.selected), ev.shiftKey)
      return
    }
    if (ev.key === 'F5') {
      ev.preventDefault()
      onRefresh()
      return
    }
    if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === 'v') {
      ev.preventDefault()
      onPaste()
      return
    }
    if (ev.key === 'F10' && ev.shiftKey) {
      const name = view[list.anchor]?.name
      const row = name ? sectionRef.current?.querySelector<HTMLElement>(`[data-row="${CSS.escape(name)}"]`) : null
      if (name && row) {
        ev.preventDefault()
        const rect = row.getBoundingClientRect()
        onRowContextMenu(view[list.anchor], {
          clientX: rect.left + 40,
          clientY: rect.top + 8,
          screenX: window.screenX + rect.left + 40,
          screenY: window.screenY + rect.top + 8,
          shiftKey: false,
        })
      }
      return
    }
  }

  const parts = tab ? crumbs(tab.path) : []

  return (
    <section
      ref={sectionRef}
      data-pane-id={pane.id}
      onMouseDown={onFocusPane}
      className={`relative flex min-h-0 min-w-0 flex-1 flex-col bg-surface ${
        active ? 'shadow-[inset_0_0_0_1.5px_rgb(96,165,250)]' : 'shadow-[inset_0_0_0_1px_var(--color-line)]'
      }`}
    >
      <div
        data-tabbar
        onDoubleClick={(ev) => {
          if ((ev.target as HTMLElement).closest('[data-tab]')) return
          onNewTab()
        }}
        className="flex h-8 shrink-0 items-stretch gap-0.5 border-b border-line bg-surface-2 px-1 pt-1"
      >
        <div className="no-bar flex min-w-0 flex-1 items-stretch gap-0.5 overflow-x-auto">
          {pane.tabIds.map((id) => {
            const tabState = tabs[id]
            if (!tabState) return null
            const selected = id === pane.activeTabId
            return (
              <div
                key={id}
                data-tab={id}
                data-tab-active={selected ? '1' : '0'}
                onPointerDown={(ev) => {
                  if (ev.button !== 0) return
                  onActivateTab(id)
                  onTabDragStart(id, ev)
                }}
                onContextMenu={(ev) => {
                  ev.preventDefault()
                  onActivateTab(id)
                  onTabContextMenu(id, pointOf(ev))
                }}
                className={`group flex h-7 max-w-[220px] shrink-0 cursor-default items-center gap-1 rounded-t-md border border-b-0 px-2 text-[11.5px] ${
                  selected
                    ? 'border-line bg-surface font-medium text-fg-strong'
                    : 'border-transparent text-muted hover:bg-surface-3'
                } ${draggingTabId === id ? 'opacity-40' : ''}`}
                title={tabState.path}
              >
                <FileIcon name={tabState.path} dir path={tabState.path} />
                <span className="max-w-[150px] truncate">
                  {tabState.path.split('\\').filter(Boolean).pop() ?? tabState.path}
                </span>
                <button
                  type="button"
                  onPointerDown={(ev) => ev.stopPropagation()}
                  onClick={(ev) => {
                    ev.stopPropagation()
                    onCloseTab(id)
                  }}
                  className="grid h-4 w-4 shrink-0 place-items-center rounded text-muted-2 hover:bg-surface-3 hover:text-fg"
                  title={t('tab.close')}
                >
                  ×
                </button>
              </div>
            )
          })}
        </div>
        <button
          type="button"
          onClick={onNewTab}
          className="mb-0.5 grid h-6 w-6 shrink-0 place-items-center self-center rounded text-muted hover:bg-surface-3 hover:text-fg"
          title={t('tab.new')}
        >
          <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.6">
            <path d="M8 3.5v9M3.5 8h9" strokeLinecap="round" />
          </svg>
        </button>
      </div>

      <div className="flex h-9 shrink-0 items-center gap-0.5 border-b border-line bg-surface px-1.5">
        <button
          type="button"
          title={t('nav.back')}
          onClick={() => tab && tab.histIdx > 0 && onNavigate(tab.id, tab.history[tab.histIdx - 1])}
          disabled={!tab || tab.histIdx <= 0}
          className="grid h-6 w-6 place-items-center rounded text-muted hover:bg-surface-3 disabled:opacity-30"
        >
          <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.6">
            <path d="M10 3 5.5 8 10 13" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        </button>
        <button
          type="button"
          title={t('nav.forward')}
          onClick={() =>
            tab && tab.histIdx < tab.history.length - 1 && onNavigate(tab.id, tab.history[tab.histIdx + 1])
          }
          disabled={!tab || tab.histIdx >= tab.history.length - 1}
          className="grid h-6 w-6 place-items-center rounded text-muted hover:bg-surface-3 disabled:opacity-30"
        >
          <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.6">
            <path d="M6 3l4.5 5L6 13" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        </button>
        <button
          type="button"
          title={t('nav.up')}
          onClick={() => tab && onNavigate(tab.id, parentPath(tab.path))}
          disabled={!tab || parentPath(tab.path) === tab.path}
          className="grid h-6 w-6 place-items-center rounded text-muted hover:bg-surface-3 disabled:opacity-30"
        >
          <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.6">
            <path d="M8 12.5V4M8 4 4.2 7.8M8 4l3.8 3.8" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        </button>

        <button
          type="button"
          title={t('nav.drives')}
          onClick={(ev) => {
            const rect = ev.currentTarget.getBoundingClientRect()
            onDriveMenu({
              clientX: rect.left,
              clientY: rect.bottom + 2,
              screenX: window.screenX + rect.left,
              screenY: window.screenY + rect.bottom + 2,
              shiftKey: false,
            })
          }}
          className="ml-1 flex h-6 shrink-0 items-center gap-0.5 rounded border border-line bg-surface-2 px-1.5 text-[11.5px] leading-none text-fg hover:bg-surface-3"
        >
          <span className="font-medium">{driveRoot(tab?.path)}</span>
          <svg viewBox="0 0 16 16" className="h-3 w-3 text-muted-2" fill="none" stroke="currentColor" strokeWidth="1.6">
            <path d="m4 6.5 4 4 4-4" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        </button>

        {addressDraft !== null ? (
          <input
            ref={addressRef}
            data-address-input
            value={addressDraft}
            onChange={(ev) => setAddressDraft(ev.target.value)}
            onBlur={() => setAddressDraft(null)}
            onKeyDown={(ev) => {
              if (ev.key === 'Enter') {
                ev.preventDefault()
                const next = addressDraft.trim()
                setAddressDraft(null)
                if (next && tab) onNavigate(tab.id, next)
              } else if (ev.key === 'Escape') {
                ev.preventDefault()
                setAddressDraft(null)
              }
            }}
            className="mx-0.5 h-6 min-w-0 flex-1 rounded border border-blue-400 bg-surface px-1.5 text-[11.5px] outline-none"
          />
        ) : (
        <div className="no-bar mx-0.5 flex h-6 min-w-0 flex-1 items-center overflow-x-auto overflow-y-hidden rounded border border-line bg-surface-2 px-1 text-[11.5px] leading-none">
          {parts.map((part, i) => (
            <div key={part.path} className="flex shrink-0 items-center">
              {i > 0 && <span className="px-0.5 text-muted-2">›</span>}
              <button
                type="button"
                onClick={() => tab && onNavigate(tab.id, part.path)}
                className={`max-w-[160px] truncate rounded px-1 py-1 leading-none hover:bg-surface-3 ${
                  i === parts.length - 1 ? 'font-medium text-fg-strong' : 'text-muted'
                }`}
              >
                {part.label}
              </button>
            </div>
          ))}
        </div>
        )}

        {showFilter && (
          <input
            value={list.filter}
            onChange={(e) => tab && update(tab.id, (d) => ({ ...d, filter: e.target.value }))}
            placeholder={t('list.filter')}
            className="h-6 w-[84px] shrink-0 rounded border border-line bg-surface-2 px-1.5 text-[11px] outline-none placeholder:text-muted-2 focus:border-blue-400 focus:bg-surface"
          />
        )}
        <button
          type="button"
          onClick={onToggleHidden}
          title={pane.hidden ? t('list.hideHidden') : t('list.showHidden')}
          className={`grid h-6 w-6 shrink-0 place-items-center rounded hover:bg-surface-3 ${
            pane.hidden ? 'text-blue-500' : 'text-muted-2'
          }`}
        >
          <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.4">
            <path d="M1.5 8s2.4-4 6.5-4 6.5 4 6.5 4-2.4 4-6.5 4-6.5-4-6.5-4Z" />
            <circle cx="8" cy="8" r="1.8" />
          </svg>
        </button>
      </div>

      <div
        className="grid h-7 shrink-0 items-center border-b border-line bg-surface-2 text-muted"
        style={{ gridTemplateColumns: grid }}
      >
        <HeaderCell label={t('list.name')} k="name" sort={pane.sort} onSort={changeSort} align="left" />
        {showSize && (
          <HeaderCell label={t('list.size')} k="size" sort={pane.sort} onSort={changeSort} align="right" />
        )}
        {showTime && (
          <HeaderCell label={t('list.mtime')} k="mtime" sort={pane.sort} onSort={changeSort} align="left" />
        )}
      </div>

      <div
        ref={scrollRef}
        tabIndex={0}
        onKeyDown={onKeyDown}
        onClick={(e) => {
          if (e.target === e.currentTarget && tab) {
            update(tab.id, (d) => ({ ...d, selected: new Set(), anchor: -1 }))
          }
        }}
        onContextMenu={(e) => {
          e.preventDefault()
          if (e.target !== e.currentTarget) return
          if (tab) update(tab.id, (d) => ({ ...d, selected: new Set(), anchor: -1 }))
          onBlankContextMenu(pointOf(e))
        }}
        className="relative min-h-0 flex-1 overflow-auto outline-none"
      >
        {list.error && (
          <div className="m-3 flex items-start gap-2 rounded-md border border-red-300 bg-red-50 p-2 text-red-800 dark:border-red-900 dark:bg-red-950/50 dark:text-red-200">
            <div className="font-mono text-[10px] font-semibold">{list.error.code}</div>
            <div className="min-w-0 flex-1 break-words text-[11px]">
              {list.error.detail || t('list.readError')}
            </div>
            <button
              type="button"
              onClick={() => tab && onNavigate(tab.id, tab.path)}
              className="rounded border border-red-300 px-1.5 py-0.5 text-[10px] hover:bg-red-100 dark:border-red-800 dark:hover:bg-red-900/40"
            >
              {t('list.retry')}
            </button>
          </div>
        )}

        <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
          {virtualizer.getVirtualItems().map((vi) => {
            const entry = view[vi.index]
            if (!entry) return null
            const dir = isDir(entry.attrs)
            const isSelected = list.selected.has(entry.name)
            return (
              <div
                key={`${entry.name}-${vi.index}`}
                data-row={entry.name}
                data-dir={dir ? '1' : '0'}
                onMouseDown={(e) => {
                  if (e.button === 0) selectRow(vi.index, e)
                }}
                onContextMenu={(e) => {
                  e.preventDefault()
                  if (!list.selected.has(entry.name)) selectRow(vi.index, e)
                  onRowContextMenu(entry, pointOf(e))
                }}
                onDoubleClick={() => activate(entry)}
                className={`absolute left-0 top-0 grid w-full items-center border-b border-line/60 pr-1.5 text-[12.5px] ${
                  isSelected ? 'bg-sel text-fg-strong' : 'hover:bg-surface-3'
                }`}
                style={{ height: ROW_H, transform: `translateY(${vi.start}px)`, gridTemplateColumns: grid }}
              >
                <div className="flex min-w-0 items-center gap-1.5 pl-1.5">
                  <FileIcon name={entry.name} dir={dir} path={joinPath(tab.path, entry.name)} />
                  <span className={`truncate ${dir ? 'font-medium text-fg-strong' : ''}`}>{entry.name}</span>
                </div>
                {showSize && (
                  <div className="tabular pr-1.5 text-right text-[11px] text-muted">
                    {formatSize(entry.size, dir)}
                  </div>
                )}
                {showTime && (
                  <div className="tabular pl-1.5 text-[11px] text-muted">{formatTime(entry.mtime)}</div>
                )}
              </div>
            )
          })}
        </div>

        {!list.error && !view.length && (
          <div className="grid h-full place-items-center text-center text-muted-2">
            {list.loading ? (
              <div className="flex flex-col items-center gap-2">
                <div className="h-4 w-4 animate-spin rounded-full border-2 border-line border-t-blue-500" />
                <div className="text-[11px]">
                  {t('list.loading')} {list.scanned > 0 && t('list.scanned', { count: list.scanned })}
                </div>
              </div>
            ) : list.filter ? (
              <div className="text-[11px]">{t('list.noMatch', { query: list.filter })}</div>
            ) : (
              <div className="text-[11px]">{t('list.empty')}</div>
            )}
          </div>
        )}
      </div>

      <div
        data-status
        className="flex h-6 shrink-0 items-center justify-between border-t border-line bg-surface-2 px-2 text-[10.5px] text-muted"
      >
        <div className="flex items-center gap-2">
          <span>{t('list.items', { count: view.length })}</span>
          {list.filter && list.entries.length !== view.length && (
            <span>{t('list.total', { count: list.entries.length })}</span>
          )}
          {list.selected.size > 0 && (
            <span>
              {t('list.selected', { count: list.selected.size })}
              {selectedSize > 0 && ` · ${formatSize(selectedSize, false)}`}
            </span>
          )}
          {list.loading && list.entries.length > 0 && (
            <span>{t('list.scanning', { count: list.scanned })}</span>
          )}
        </div>
        <span className="max-w-[45%] truncate font-mono text-[9.5px] text-muted-2">{tab?.path}</span>
      </div>

      {dropHint && (
        <div className="pointer-events-none absolute inset-0 z-20">
          {dropHint === 'center' ? (
            <div className="absolute inset-0 border-2 border-blue-400 bg-blue-400/10" />
          ) : (
            <div className={`absolute bg-blue-400/25 outline outline-2 outline-blue-400 ${HINT_STYLE[dropHint]}`} />
          )}
        </div>
      )}
    </section>
  )
}

function HeaderCell({
  label,
  k,
  sort,
  onSort,
  align,
}: {
  label: string
  k: SortKey
  sort: SortState
  onSort: (k: SortKey) => void
  align: 'left' | 'right'
}) {
  const active = sort.key === k
  return (
    <button
      type="button"
      onClick={() => onSort(k)}
      className={`flex h-full items-center gap-1 px-1.5 text-[10.5px] font-medium tracking-wide text-muted hover:text-fg ${
        align === 'right' ? 'justify-end' : 'justify-start'
      }`}
    >
      {label}
      <span className={`text-[8px] ${active ? 'text-blue-500' : 'text-transparent'}`}>
        {sort.dir === 'asc' ? '▲' : '▼'}
      </span>
    </button>
  )
}

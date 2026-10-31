import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { crumbs, isArchiveName, isDir, isInsideArchive, joinPath, parentPath, underPath } from './paths'
import { buildGitBadges } from './git'
import { HeaderCell, ColDivider } from './components/TableHeader'
import { AddressBar, type AddressBarHandle } from './components/AddressBar'
import { DrivesOverview } from './components/DrivesOverview'
import { PaneTabBar } from './components/PaneTabBar'
import { PaneStatusBar } from './components/PaneStatusBar'
import { usePaneColumnResize } from './hooks/usePaneColumnResize'
import { FileTableRow } from './components/FileTableRow'
import type { ShortcutId } from './shortcuts'
import { usePaneKeyboard } from './hooks/usePaneKeyboard'
import { EMPTY_LIST, filterList, filterMatches } from './types'
import type { Drive, Entry, GitStatus, Lang, ListData, SortKey, SortState } from './types'
import { makeT } from './i18n'
import {
  BASE_FONT_SIZE,
  DEFAULT_LIST_FONT_SIZE,
  DEFAULT_NAV_FONT_SIZE,
  DEFAULT_TAB_FONT_SIZE,
  clampIconSize,
  clampRegionFontSize,
} from './fonts'
import type { DropMode, PaneState, TabState } from './workspace'

/** Metrics at the default 13 px text size; every pane scales them by the settings font size. */
const ROW_H = 26
/** List header row height at the default text size. */
const HEADER_H = 24

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
const isBlankListClick = (e: React.MouseEvent<HTMLElement>): boolean => {
  const rect = e.currentTarget.getBoundingClientRect()
  const isScrollbar =
    e.clientX >= rect.left + e.currentTarget.clientLeft + e.currentTarget.clientWidth ||
    e.clientY >= rect.top + e.currentTarget.clientTop + e.currentTarget.clientHeight
  return !isScrollbar && !(e.target instanceof Element && e.target.closest('[data-row], [data-drive], button, input, a'))
}

type Props = {
  pane: PaneState
  tabs: Record<string, TabState>
  data: ListData | undefined
  /** Repository status of the active tab's folder; null when unknown or not a repo. */
  git: GitStatus | null
  active: boolean
  lang: Lang
  /** File and drive list icon size in px (14–32). */
  iconSize: number
  /** Tab-strip text size in px; drives strip height and tab icons. */
  tabFontSize: number
  /** File-list text size in px; drives rows, columns and icons. */
  listFontSize: number
  /** Navigation-bar text size in px; drives the toolbar controls. */
  navFontSize: number
  /** File-list "size" column switch; a narrow pane still drops it on its own. */
  showColSize: boolean
  /** File-list "type" column switch. */
  showColType: boolean
  /** File-list "modified" column switch. */
  showColTime: boolean
  /** Stored name column width in px at the base font size; `0` fills the pane. */
  listNameWidth: number
  /** Persists the name column width after a drag; `0` restores the fill-the-pane layout. */
  onListNameWidth: (px: number) => void
  /** Stored "size" column width in px at the base font size; shared by every pane. */
  colSize: number
  /** Stored "type" column width in px at the base font size; shared by every pane. */
  colType: number
  /** Stored "modified" column width in px at the base font size; shared by every pane. */
  colTime: number
  /** Stored drive-overview "capacity" column width in px at the base font size. */
  colDriveCapacity: number
  /** Stored drive-overview "free" column width in px at the base font size. */
  colDriveFree: number
  /** Persists every column width after a drag or a double-click reset. */
  onColWidths: (size: number, type: number, time: number, capacity: number, free: number) => void
  dropHint: DropMode | null
  draggingTabId: string | null
  onFocusPane: () => void
  onNewTab: () => void
  onCloseTab: (tabId: string) => void
  onActivateTab: (tabId: string) => void
  /** Opens `path` in a background tab; middle-click and Ctrl+click land here. */
  onOpenInNewTab: (path: string) => void
  /** Whether those gestures open a tab at all; off means they act like a plain click. */
  newTabOnMiddleClick: boolean
  /** Opens a file or non-filesystem drive root with the shell, reporting failures. */
  onOpenTarget: (path: string) => void
  onNavigate: (tabId: string, path: string) => void
  /** Steps the tab's history cursor without touching the stack (back/forward). */
  onHistoryMove: (tabId: string, delta: number) => void
  onTabDragStart: (tabId: string, ev: React.PointerEvent) => void
  onSortChange: (sort: SortState) => void
  onToggleHidden: () => void
  onClipboard: (cut: boolean) => void
  /** Copies the tab's folder path verbatim; the breadcrumb button calls this. */
  onCopyPath: (path: string) => void
  /** Reports an address-bar edit that did not resolve to a folder, so the hint is visible. */
  onPathInvalid: (path: string) => void
  onRowContextMenu: (entry: Entry, point: MenuPoint) => void
  /** Starts a pointer-based in-app drag of the current selection/row. */
  onRowDragStart: (entry: Entry, ev: React.PointerEvent) => void
  onBlankContextMenu: (point: MenuPoint) => void
  onTabContextMenu: (tabId: string, point: MenuPoint) => void
  drives: Drive[]
  onDriveMenu: (point: MenuPoint) => void
  /** Right-click on one overview row: properties, and eject for removable media. */
  onDriveRowMenu: (drive: Drive, point: MenuPoint) => void
  onProperties: (names: string[]) => void
  onRename: (name: string) => void
  /** Opens the batch dialog for a multi-selection. */
  onRenameMany: (names: string[]) => void
  onDelete: (names: string[], permanent: boolean) => void
  onPaste: () => void
  onRefresh: () => void
  /** Rescans the tab after a failed listing; unlike navigation, a same-path request must run. */
  onRetry: (tabId: string) => void
  /** Resolved key combinations, so rebinding in settings reaches the pane commands. */
  keys: Record<ShortcutId, string>
  onNewFolder?: () => void
  onCyclePane?: () => void
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
  git,
  active,
  lang,
  iconSize,
  tabFontSize,
  listFontSize,
  navFontSize,
  showColSize,
  showColType,
  showColTime,
  listNameWidth,
  onListNameWidth,
  colSize,
  colType,
  colTime,
  colDriveCapacity,
  colDriveFree,
  onColWidths,
  dropHint,
  draggingTabId,
  onFocusPane,
  onNewTab,
  onCloseTab,
  onActivateTab,
  onOpenInNewTab,
  newTabOnMiddleClick,
  onOpenTarget,
  onNavigate,
  onHistoryMove,
  onTabDragStart,
  onSortChange,
  onToggleHidden,
  onClipboard,
  onCopyPath,
  onPathInvalid,
  onRowContextMenu,
  onRowDragStart,
  onBlankContextMenu,
  onTabContextMenu,
  drives,
  onDriveMenu,
  onDriveRowMenu,
  onProperties,
  onRename,
  onRenameMany,
  onDelete,
  onPaste,
  onRefresh,
  onRetry,
  keys,
  onNewFolder,
  onCyclePane,
  update,
}: Props) {
  const t = makeT(lang)
  const list = data ?? EMPTY_LIST
  const tab = tabs[pane.activeTabId]
  /** Empty path = the drive overview, rendered from `drives` instead of a scan. */
  const root = !tab?.path
  /** Network locations are top-level rows: their root climbs straight back to the overview. */
  const location = tab?.path
    ? drives.find((drive) => drive.kind === 'location' && underPath(tab.path, drive.root))
    : undefined
  const atLocationRoot = !!location && tab?.path.toLowerCase() === location.root.toLowerCase()
  const upTarget = atLocationRoot ? '' : parentPath(tab?.path ?? '')
  const scrollRef = useRef<HTMLDivElement>(null)
  /** Header grid outside the scroll port; its transform mirrors the list's scrollLeft. */
  const headerRef = useRef<HTMLDivElement>(null)
  const sectionRef = useRef<HTMLElement>(null)
  const [width, setWidth] = useState(0)
  /** Type-ahead buffer plus the hidden input that receives IME composition. */
  const addressBarRef = useRef<AddressBarHandle>(null)
  /** Tab strip scroller and whether its tabs no longer fit, which arms the overflow UI. */
  const stripRef = useRef<HTMLDivElement>(null)
  const [stripOverflow, setStripOverflow] = useState(false)
  const [tabListOpen, setTabListOpen] = useState(false)

  useEffect(() => {
    const strip = stripRef.current
    if (!strip) return
    const measure = () => setStripOverflow(strip.scrollWidth > strip.clientWidth + 1)
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(strip)
    return () => observer.disconnect()
  }, [pane.tabIds, tabs])

  // The active tab must stay on screen: with 20+ tabs it would otherwise open offscreen.
  useEffect(() => {
    const strip = stripRef.current
    const item = strip?.querySelector<HTMLElement>(`[data-tab="${CSS.escape(pane.activeTabId)}"]`)
    if (!strip || !item) return
    const left = item.offsetLeft
    const right = left + item.offsetWidth
    if (left < strip.scrollLeft) strip.scrollLeft = left
    else if (right > strip.scrollLeft + strip.clientWidth) strip.scrollLeft = right - strip.clientWidth
  }, [pane.activeTabId, pane.tabIds, stripOverflow])

  useEffect(() => {
    const el = sectionRef.current
    if (!el) return
    const observer = new ResizeObserver((entries) => setWidth(entries[0].contentRect.width))
    observer.observe(el)
    return () => observer.disconnect()
  }, [])

  // Each surface scales from its own text size. The global size still drives rem spacing;
  // the list scale also keeps stored column widths proportional to the list text.
  const tabFont = clampRegionFontSize(tabFontSize, DEFAULT_TAB_FONT_SIZE)
  const listFont = clampRegionFontSize(listFontSize, DEFAULT_LIST_FONT_SIZE)
  const navFont = clampRegionFontSize(navFontSize, DEFAULT_NAV_FONT_SIZE)
  const listScale = listFont / BASE_FONT_SIZE
  const iconPx = clampIconSize(iconSize)
  const tabBarHeight = Math.round(tabFont * 1.75) + 3
  const navBarHeight = Math.round(navFont * 2)
  const navControlHeight = navBarHeight - 4
  const rowHeight = Math.max(Math.round(ROW_H * listScale), iconPx + 8)
  const headerHeight = Math.round(HEADER_H * listScale)
  const {
    nameWidth,
    listMinWidth,
    listSpan,
    showSize,
    showType,
    showTime,
    showFilter,
    showDriveLabel,
    showDriveCapacity,
    driveGrid,
    grid,
    startColDrag,
    resetCol,
    startNameDrag,
  } = usePaneColumnResize({
    width,
    listScale,
    showColSize,
    showColType,
    showColTime,
    colSize,
    colType,
    colTime,
    colDriveCapacity,
    colDriveFree,
    listNameWidth,
    headerRef,
    scrollRef,
    onFocusPane,
    onColWidths,
    onListNameWidth,
  })
  const navButtonStyle = { width: navControlHeight, height: navControlHeight }
  const navFieldStyle = { height: navControlHeight }

  const view = useMemo(
    () => (list.filter.trim() ? list.entries.filter((e) => filterMatches(e.name, list.filter)) : list.entries),
    [list.entries, list.filter],
  )

  /** The anchor is a name, so it must be resolved against the current view every time. */
  const anchorIdx = useMemo(
    () => (list.anchor ? view.findIndex((e) => e.name === list.anchor) : -1),
    [view, list.anchor],
  )

  /** Names the filter currently shows; rebuilt only when the visible view changes. */
  const viewNameSet = useMemo(() => new Set(view.map((entry) => entry.name)), [view])

  /**
   * Operations only ever touch rows the user can see. The filter input prunes the state, so
   * this intersection is the second line of defence for a selection that outlived its view.
   */
  const visibleSelected = useMemo(() => {
    const picked = list.selected
    if (!picked.size) return picked
    // Common case: nothing is hidden, so the selection set itself is already the answer and
    // a 10k-item selection costs no allocation at all.
    let hidden = 0
    for (const name of picked) if (!viewNameSet.has(name)) hidden += 1
    if (!hidden) return picked
    const next = new Set<string>()
    for (const name of picked) if (viewNameSet.has(name)) next.add(name)
    return next
  }, [viewNameSet, list.selected])

  const virtualizer = useVirtualizer({
    count: view.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => rowHeight,
    overscan: 10,
  })

  // Constant-size rows: a new estimate must invalidate the cached offsets, otherwise the
  // scrollbar and spacer keep the old row height after the font size changes.
  useEffect(() => {
    virtualizer.measure()
  }, [rowHeight, virtualizer])

  /**
   * The list's vertical scrollbar eats into its content box while the header stays full
   * width. Reclaim that strip so the header columns sit exactly over the row columns.
   */
  const [gutter, setGutter] = useState(0)
  useLayoutEffect(() => {
    const el = scrollRef.current
    if (!el) return
    setGutter(el.offsetWidth - el.clientWidth)
  }, [view.length, list.loading, list.error, rowHeight, width])

  const selectedSize = useMemo(
    () =>
      list.entries.reduce(
        (sum, e) => (visibleSelected.has(e.name) && !isDir(e.attrs) ? sum + e.size : sum),
        0,
      ),
    [list.entries, visibleSelected],
  )

  // Rebuilt only when a new status payload arrives, never per row.
  const gitBadges = useMemo(() => buildGitBadges(git), [git])

  const activate = (entry: Entry) => {
    if (!tab) return
    const full = joinPath(tab.path, entry.name)
    const shouldNavigate = isDir(entry.attrs) || (!isInsideArchive(tab.path) && isArchiveName(entry.name))
    if (shouldNavigate) onNavigate(tab.id, full)
    else onOpenTarget(full)
  }

  const changeSort = (key: SortKey) => {
    const dir = pane.sort.key === key && pane.sort.dir === 'asc' ? 'desc' : 'asc'
    onSortChange({ key, dir })
  }


  const { typed, typeAheadRef, typeAhead, selectRow, onKeyDown } = usePaneKeyboard({
    tab,
    view,
    anchorIdx,
    visibleSelected,
    rowHeight,
    keys,
    scrollRef,
    sectionRef,
    addressBarRef,
    virtualizer,
    upTarget,
    update,
    activate,
    onProperties,
    onHistoryMove,
    onNavigate,
    onClipboard,
    onRename,
    onRenameMany,
    onDelete,
    onRefresh,
    onPaste,
    onRowContextMenu,
    onBlankContextMenu,
    onNewFolder,
    onCopyPath,
    onCyclePane,
  })

  // Inside a location the first crumb is the location itself, never the WebDAV share root.
  const parts = location
    ? [
        { label: location.name, path: location.root },
        ...crumbs(tab?.path ?? '')
          .filter((crumb) => underPath(crumb.path, location.root))
          .slice(1),
      ]
    : tab?.path
      ? crumbs(tab.path)
      : []

  return (
    <section
      ref={sectionRef}
      data-pane-id={pane.id}
      onMouseDown={onFocusPane}
      className={`relative flex min-h-0 min-w-0 flex-1 flex-col bg-surface ${
        active ? 'shadow-[inset_0_0_0_1.5px_rgb(96,165,250)]' : 'shadow-[inset_0_0_0_1px_var(--color-line)]'
      }`}
    >
      <PaneTabBar
        pane={pane}
        tabs={tabs}
        active={active}
        tabBarHeight={tabBarHeight}
        t={t}
        stripRef={stripRef}
        stripOverflow={stripOverflow}
        tabListOpen={tabListOpen}
        setTabListOpen={setTabListOpen}
        onActivateTab={onActivateTab}
        onCloseTab={onCloseTab}
        onNewTab={onNewTab}
        onTabDragStart={onTabDragStart}
        onTabContextMenu={onTabContextMenu}
        pointOf={pointOf}
        draggingTabId={draggingTabId}
      />

      <div
        className="typo-nav flex shrink-0 items-center gap-0.5 border-b border-line bg-surface px-1"
        style={{ height: navBarHeight }}
      >
        <AddressBar
          ref={addressBarRef}
          tab={tab}
          keys={keys}
          t={t}
          location={location}
          root={root}
          parts={parts}
          upTarget={upTarget}
          navButtonStyle={navButtonStyle}
          navFieldStyle={navFieldStyle}
          onHistoryMove={onHistoryMove}
          onNavigate={onNavigate}
          onDriveMenu={onDriveMenu}
          onCopyPath={onCopyPath}
          onPathInvalid={onPathInvalid}
          onOpenInNewTab={onOpenInNewTab}
          newTabOnMiddleClick={newTabOnMiddleClick}
        />

        {showFilter && !root && (
          <input
            value={list.filter}
            onChange={(e) => tab && update(tab.id, (d) => filterList(d, e.target.value))}
            placeholder={t('list.filter')}
            style={navFieldStyle}
            className="w-[5.25rem] shrink-0 rounded border border-line bg-surface-2 px-1.5 text-[0.92em] outline-none placeholder:text-muted-2 focus:border-blue-400 focus:bg-surface"
          />
        )}
        {!showFilter && !root && list.filter && (
          // The input is gone at this width, but the filter still decides what the list
          // shows: keep a chip so it is never invisible and never sticky.
          <button
            type="button"
            data-filter-clear
            onClick={() => tab && update(tab.id, (d) => filterList(d, ''))}
            title={t('list.filterActive', { query: list.filter })}
            style={navFieldStyle}
            className="ml-0.5 flex max-w-[5rem] shrink-0 items-center gap-0.5 rounded border border-line bg-surface-3 px-1 text-[0.85em] text-fg"
          >
            <span className="truncate">{list.filter}</span>
            <span className="shrink-0 text-muted">×</span>
          </button>
        )}
        <button
          type="button"
          onClick={onToggleHidden}
          title={pane.hidden ? t('list.hideHidden') : t('list.showHidden')}
          style={navButtonStyle}
          className={`grid shrink-0 place-items-center rounded hover:bg-surface-3 ${
            pane.hidden ? 'text-blue-500' : 'text-muted-2'
          }`}
        >
          <svg viewBox="0 0 16 16" className="h-[1.17em] w-[1.17em]" fill="none" stroke="currentColor" strokeWidth="1.4">
            <path d="M1.5 8s2.4-4 6.5-4 6.5 4 6.5 4-2.4 4-6.5 4-6.5-4-6.5-4Z" />
            <circle cx="8" cy="8" r="1.8" />
          </svg>
        </button>
      </div>

      {!root && (
        <div
          className="relative shrink-0 overflow-hidden"
          style={{ height: headerHeight, marginRight: gutter }}
        >
          <div
            ref={headerRef}
            className="typo-list relative grid h-full items-center border-b border-line bg-surface-2 pr-1.5 text-muted"
            style={{ gridTemplateColumns: grid, minWidth: listMinWidth, width: listSpan }}
          >
            <HeaderCell label={t('list.name')} k="name" sort={pane.sort} onSort={changeSort} align="left" />
            {showSize && (
              <div className="relative flex h-full min-w-0">
                <HeaderCell label={t('list.size')} k="size" sort={pane.sort} onSort={changeSort} align="right" />
                <ColDivider
                  label={t('list.size')}
                  onPointerDown={(ev) => startColDrag('size', ev)}
                  onDoubleClick={() => resetCol('size')}
                />
              </div>
            )}
            {showType && (
              <div className="relative flex h-full min-w-0">
                <HeaderCell label={t('list.type')} k="type" sort={pane.sort} onSort={changeSort} align="left" />
                <ColDivider
                  label={t('list.type')}
                  onPointerDown={(ev) => startColDrag('type', ev)}
                  onDoubleClick={() => resetCol('type')}
                />
              </div>
            )}
            {showTime && (
              <div className="relative flex h-full min-w-0">
                <HeaderCell label={t('list.mtime')} k="mtime" sort={pane.sort} onSort={changeSort} align="left" />
                <ColDivider
                  label={t('list.mtime')}
                  onPointerDown={(ev) => startColDrag('time', ev)}
                  onDoubleClick={() => resetCol('time')}
                />
              </div>
            )}
          </div>
          {/* Pinned to the row's right edge, or to the visible edge while the list scrolls
              horizontally, so the handle is always reachable. */}
          <span
            role="separator"
            aria-orientation="vertical"
            aria-label={t('list.nameWidth')}
            title={t('list.nameWidthHint')}
            data-gutter-handle
            onPointerDown={startNameDrag}
            onDoubleClick={() => onListNameWidth(0)}
            style={{ left: nameWidth > 0 ? `min(${listMinWidth}, 100%)` : '100%' }}
            className="absolute inset-y-0 z-10 w-1.5 -translate-x-full cursor-col-resize touch-none select-none hover:bg-blue-400/60 active:bg-blue-500"
          />
        </div>
      )}

      {root && (
        <div
          className="typo-list grid shrink-0 items-center border-b border-line bg-surface-2 text-muted"
          style={{ height: headerHeight, gridTemplateColumns: driveGrid }}
        >
          <span className="truncate px-1.5 text-[0.84em] font-medium tracking-wide">{t('list.name')}</span>
          {showDriveLabel && (
            <span className="truncate px-1.5 text-[0.84em] font-medium tracking-wide">{t('list.volumeLabel')}</span>
          )}
          <span className="truncate px-1.5 text-[0.84em] font-medium tracking-wide">{t('list.type')}</span>
          {showDriveCapacity && (
            <>
              <div className="relative flex h-full min-w-0 items-center justify-end">
                <span className="truncate px-1.5 text-right text-[0.84em] font-medium tracking-wide">
                  {t('list.capacity')}
                </span>
                <ColDivider
                  label={t('list.capacity')}
                  onPointerDown={(ev) => startColDrag('capacity', ev)}
                  onDoubleClick={() => resetCol('capacity')}
                />
              </div>
              <div className="relative flex h-full min-w-0 items-center justify-end">
                <span className="truncate px-1.5 text-right text-[0.84em] font-medium tracking-wide">
                  {t('list.free')}
                </span>
                <ColDivider
                  label={t('list.free')}
                  onPointerDown={(ev) => startColDrag('free', ev)}
                  onDoubleClick={() => resetCol('free')}
                />
              </div>
            </>
          )}
        </div>
      )}

      {!root && list.truncated && (
        <div
          data-truncated
          className="flex shrink-0 items-center gap-1 border-b border-amber-300 bg-amber-50 px-2 py-1 text-[0.75em] text-amber-800 dark:border-amber-800 dark:bg-amber-950/40 dark:text-amber-200"
        >
          {t('list.truncated', { count: list.entries.length })}
        </div>
      )}

      <div
        ref={scrollRef}
        tabIndex={0}
        data-list
        onKeyDown={onKeyDown}
        onScroll={(ev) => {
          // The header lives outside the scroll port; mirror the offset so its columns stay
          // over the rows while the file list scrolls horizontally.
          const header = headerRef.current
          if (header) header.style.transform = `translateX(${-ev.currentTarget.scrollLeft}px)`
        }}
        onFocus={(ev) => {
          // IME composition needs an editable target, so the list hands focus to a hidden
          // input. Keys still bubble back to this container, so every handler is unchanged.
          if (ev.target === scrollRef.current) typeAheadRef.current?.focus({ preventScroll: true })
        }}
        onClick={(e) => {
          if (!isBlankListClick(e)) return
          if (tab) update(tab.id, (d) => ({ ...d, selected: new Set(), anchor: '' }))
        }}
        onContextMenu={(e) => {
          e.preventDefault()
          if (!isBlankListClick(e)) return
          if (tab) update(tab.id, (d) => ({ ...d, selected: new Set(), anchor: '' }))
          onBlankContextMenu(pointOf(e))
        }}
        data-drop-dir={tab?.path || undefined}
        className="typo-list relative min-h-0 flex-1 overflow-auto outline-none"
        // Blank strip under the last row: the container is the click target there, so a
        // right-click reaches the folder menu even when the list fills the pane.
        style={{ paddingBottom: rowHeight * 2 }}
      >
        <input
          ref={typeAheadRef}
          data-typeahead-input
          tabIndex={-1}
          aria-label={t('list.typeAhead')}
          autoComplete="off"
          spellCheck={false}
          onCompositionEnd={(ev) => {
            const text = ev.data
            // Stay empty: printable keys are handled by the pane, only IME text lands here.
            ev.currentTarget.value = ''
            if (text) typeAhead(text)
          }}
          className="pointer-events-none absolute left-0 top-0 h-px w-px border-0 p-0 opacity-0"
        />
        {list.error && !root && (
          <div className="m-3 flex items-start gap-2 rounded-md border border-red-300 bg-red-50 p-2 text-red-800 dark:border-red-900 dark:bg-red-950/50 dark:text-red-200">
            <div className="font-mono text-[0.8em] font-semibold">{list.error.code}</div>
            <div className="min-w-0 flex-1 break-words text-[0.88em]">
              {list.error.detail || t('list.readError')}
            </div>
            <button
              type="button"
              onClick={() => tab && onRetry(tab.id)}
              className="rounded border border-red-300 px-1.5 py-0.5 text-[0.8em] hover:bg-red-100 dark:border-red-800 dark:hover:bg-red-900/40"
            >
              {t('list.retry')}
            </button>
          </div>
        )}

        {root && (
          <DrivesOverview
            drives={drives}
            tab={tab}
            rowHeight={rowHeight}
            driveGrid={driveGrid}
            showDriveLabel={showDriveLabel}
            showDriveCapacity={showDriveCapacity}
            listScale={listScale}
            iconPx={iconPx}
            t={t}
            onNavigate={onNavigate}
            onOpenTarget={onOpenTarget}
            onDriveRowMenu={onDriveRowMenu}
            pointOf={pointOf}
          />
        )}

        {!root && (
        <div style={{ height: virtualizer.getTotalSize(), position: 'relative', minWidth: listMinWidth, width: listSpan }}>
          {virtualizer.getVirtualItems().map((vi) => {
            const entry = view[vi.index]
            if (!entry) return null
            const isSelected = list.selected.has(entry.name)
            return (
              <FileTableRow
                key={`${entry.name}-${vi.index}`}
                entry={entry}
                index={vi.index}
                tabPath={tab.path}
                rowHeight={rowHeight}
                startOffset={vi.start}
                grid={grid}
                isSelected={isSelected}
                listScale={listScale}
                iconPx={iconPx}
                gitBadges={gitBadges}
                t={t}
                showSize={showSize}
                showType={showType}
                showTime={showTime}
                newTabOnMiddleClick={newTabOnMiddleClick}
                selectRow={selectRow}
                onRowDragStart={onRowDragStart}
                onOpenInNewTab={onOpenInNewTab}
                onRowContextMenu={onRowContextMenu}
                onDoubleClick={() => activate(entry)}
                pointOf={pointOf}
                isRowSelected={list.selected.has(entry.name)}
              />
            )
          })}
        </div>
        )}

        {!root && !list.error && !view.length && (
          <div className="grid h-full place-items-center text-center text-muted-2">
            {list.loading ? (
              <div className="flex flex-col items-center gap-2">
                <div className="h-4 w-4 animate-spin rounded-full border-2 border-line border-t-blue-500" />
                <div className="text-[0.88em]">
                  {t('list.loading')} {list.scanned > 0 && t('list.scanned', { count: list.scanned })}
                </div>
              </div>
            ) : list.filter ? (
              <div className="text-[0.88em]">{t('list.noMatch', { query: list.filter })}</div>
            ) : (
              <div className="text-[0.88em]">{t('list.empty')}</div>
            )}
          </div>
        )}
      </div>

      <PaneStatusBar
        git={git}
        t={t}
        root={root}
        driveCount={drives.length}
        visibleCount={view.length}
        entryCount={list.entries.length}
        filter={list.filter}
        selectedCount={visibleSelected.size}
        selectedSize={selectedSize}
        loading={list.loading}
        scanned={list.scanned}
        path={tab?.path}
      />

      {typed && (
        <div
          data-typeahead-hint
          className="pointer-events-none absolute bottom-8 left-1/2 z-20 -translate-x-1/2 rounded border border-line bg-surface-3/95 px-2 py-0.5 text-[0.75em] font-medium text-fg-strong shadow"
        >
          {typed}
        </div>
      )}

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

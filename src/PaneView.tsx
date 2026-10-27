import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { FileIcon } from './icons'
import { crumbs, formatSize, formatTime, isDir, isFsPath, joinPath, parentPath, pathKey, resolveAddress, underPath } from './paths'
import { buildGitBadges } from './git'
import type { GitBadges } from './git'
import { openPath, pathIsDir } from './api'
import { matchBinding } from './shortcuts'
import type { ShortcutId } from './shortcuts'
import { matchTypeAhead } from './typeahead'
import { EMPTY_LIST, filterList, filterMatches } from './types'
import { DEFAULT_SETTINGS } from './persist'
import type { Drive, Entry, GitStatus, Lang, ListData, SortKey, SortState } from './types'
import { driveKindLabel, makeT } from './i18n'
import {
  BASE_FONT_SIZE,
  DEFAULT_LIST_FONT_SIZE,
  DEFAULT_NAV_FONT_SIZE,
  DEFAULT_TAB_FONT_SIZE,
  clampRegionFontSize,
} from './fonts'
import type { Translate } from './i18n'
import type { DropMode, PaneState, TabState } from './workspace'

/** Drive label for the address bar ("C:", "D:"); UNC and relative paths fall back to a dash. */
const driveRoot = (path: string | undefined): string => {
  const match = path ? /^([A-Za-z]:)/.exec(path) : null
  return match ? match[1].toUpperCase() : '⋯'
}

/** Badge colour per porcelain letter; conflicts and deletions read as problems. */
const gitTone = (code: string) =>
  code === 'D' || code === '!'
    ? 'text-red-500'
    : code === 'A' || code === 'U'
      ? 'text-emerald-500'
      : code === 'R' || code === 'C'
        ? 'text-sky-500'
        : 'text-amber-500'

/** Metrics at the default 13 px text size; every pane scales them by the settings font size. */
const ROW_H = 26
/** List header row height at the default text size. */
const HEADER_H = 24
/** Column bounds in px at the default font size; the stored widths are clamped to them. */
const MIN_COL_SIZE = 56
/** A full `YYYY-MM-DD HH:mm` needs this much at the default 13 px list size. */
const MIN_COL_TIME = 120
/** "123.4 GB / 465.7 GB" plus the usage bar needs more room than a plain file size. */
const MIN_DRIVE_CAPACITY = 120
const MIN_DRIVE_FREE = 56
/** Drive "type" column; fixed, since the label is short and never wraps. */
const DRIVE_TYPE_COL = 72
const MAX_COL_WIDTH = 480
/** Name column floor, so dragging a divider can never starve the file names. */
const MIN_COL_NAME = 140
const MIN_WIDTH_FILTER = 420

/** Header dividers govern the column on their right; the drive grid reuses the same drag. */
type ColKey = 'size' | 'time' | 'capacity' | 'free'

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
  /** Repository status of the active tab's folder; null when unknown or not a repo. */
  git: GitStatus | null
  active: boolean
  lang: Lang
  /** Tab-strip text size in px; drives strip height and tab icons. */
  tabFontSize: number
  /** File-list text size in px; drives rows, columns and icons. */
  listFontSize: number
  /** Navigation-bar text size in px; drives the toolbar controls. */
  navFontSize: number
  /** Stored "size" column width in px at the base font size; shared by every pane. */
  colSize: number
  /** Stored "modified" column width in px at the base font size; shared by every pane. */
  colTime: number
  /** Stored drive-overview "capacity" column width in px at the base font size. */
  colDriveCapacity: number
  /** Stored drive-overview "free" column width in px at the base font size. */
  colDriveFree: number
  /** Persists every column width after a drag or a double-click reset. */
  onColWidths: (size: number, time: number, capacity: number, free: number) => void
  dropHint: DropMode | null
  draggingTabId: string | null
  onFocusPane: () => void
  onNewTab: () => void
  onCloseTab: (tabId: string) => void
  onActivateTab: (tabId: string) => void
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
  tabFontSize,
  listFontSize,
  navFontSize,
  colSize,
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
  /** Live column widths while a header divider is dragged; committed once on release. */
  const [draft, setDraft] = useState<{
    size: number
    time: number
    capacity: number
    free: number
  } | null>(null)
  /** Full-path editor: the draft text, whether the last Enter missed, and its request token. */
  const [addressDraft, setAddressDraft] = useState<string | null>(null)
  const [addressBad, setAddressBad] = useState(false)
  const addressRef = useRef<HTMLInputElement>(null)
  /** Invalidates an in-flight existence check once the editor closes or commits again. */
  const addressSeq = useRef(0)
  /** Type-ahead buffer plus the hidden input that receives IME composition. */
  const [typed, setTyped] = useState('')
  const typeAheadRef = useRef<HTMLInputElement>(null)
  const typeAheadTimer = useRef<number | null>(null)

  useEffect(
    () => () => {
      if (typeAheadTimer.current) window.clearTimeout(typeAheadTimer.current)
    },
    [],
  )

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

  // Each surface scales from its own text size. The global size still drives rem spacing;
  // the list scale also keeps stored column widths proportional to the list text.
  const tabFont = clampRegionFontSize(tabFontSize, DEFAULT_TAB_FONT_SIZE)
  const listFont = clampRegionFontSize(listFontSize, DEFAULT_LIST_FONT_SIZE)
  const navFont = clampRegionFontSize(navFontSize, DEFAULT_NAV_FONT_SIZE)
  const tabScale = tabFont / BASE_FONT_SIZE
  const listScale = listFont / BASE_FONT_SIZE
  const tabBarHeight = Math.round(tabFont * 1.75) + 3
  const tabItemHeight = tabBarHeight - 3
  const navBarHeight = Math.round(navFont * 2)
  const navControlHeight = navBarHeight - 4
  const rowHeight = Math.round(ROW_H * listScale)
  const headerHeight = Math.round(HEADER_H * listScale)
  const nameMin = Math.round(MIN_COL_NAME * listScale)
  const minSizeCol = Math.round(MIN_COL_SIZE * listScale)
  const minTimeCol = Math.round(MIN_COL_TIME * listScale)
  // Dragged widths stay local until release, so one gesture writes settings once.
  const wantSize = Math.round((draft?.size ?? colSize) * listScale)
  const wantTime = Math.round((draft?.time ?? colTime) * listScale)
  // Columns keep their stored width. A narrow pane scrolls horizontally instead of
  // squeezing the neighbour, so dragging the size column can never deform the time column.
  const showSize = width === 0 || width >= nameMin + minSizeCol
  const showTime = width === 0 || width >= nameMin + minSizeCol + minTimeCol
  // Stored widths below the floor are lifted so a legacy narrow column cannot wrap its text.
  const sizeCol = Math.max(minSizeCol, wantSize)
  const timeCol = Math.max(minTimeCol, wantTime)
  // Rows carry `pr-1.5` that the header repeats, so the scroll range covers both without
  // clipping the last column; `0.375rem` is that padding at the root font size.
  const listWidth = nameMin + (showSize ? sizeCol : 0) + (showTime ? timeCol : 0)
  const listMinWidth = `calc(${listWidth}px + 0.375rem)`
  const showFilter = width === 0 || width >= Math.round(MIN_WIDTH_FILTER * listScale)
  // The overview drops the label column first, then the capacity pair, the same way the
  // file list drops size and modified time on a narrow pane.
  const showDriveLabel = width === 0 || width >= 300
  // The drive grid shares the list's scale, so a stored width means the same at any font.
  const driveTypeCol = Math.round(DRIVE_TYPE_COL * listScale)
  const minDriveCapacity = Math.round(MIN_DRIVE_CAPACITY * listScale)
  const minDriveFree = Math.round(MIN_DRIVE_FREE * listScale)
  const wantCapacity = Math.round((draft?.capacity ?? colDriveCapacity) * listScale)
  const wantFree = Math.round((draft?.free ?? colDriveFree) * listScale)
  const showDriveCapacity =
    width === 0 || width >= nameMin + driveTypeCol + minDriveCapacity + minDriveFree
  const capacityCol =
    width === 0
      ? wantCapacity
      : Math.min(
          wantCapacity,
          Math.max(minDriveCapacity, width - nameMin - driveTypeCol - (showDriveCapacity ? minDriveFree : 0)),
        )
  const freeCol =
    width === 0
      ? wantFree
      : Math.min(wantFree, Math.max(minDriveFree, width - nameMin - driveTypeCol - capacityCol))
  const driveGrid = [
    'minmax(0,2fr)',
    ...(showDriveLabel ? ['minmax(0,1fr)'] : []),
    `${driveTypeCol}px`,
    ...(showDriveCapacity ? [`${capacityCol}px`, `${freeCol}px`] : []),
  ].join(' ')
  const grid = showTime
    ? `minmax(${nameMin}px, 1fr) ${sizeCol}px ${timeCol}px`
    : showSize
      ? `minmax(${nameMin}px, 1fr) ${sizeCol}px`
      : `minmax(${nameMin}px, 1fr)`
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

  /**
   * Operations only ever touch rows the user can see. The filter input prunes the state, so
   * this intersection is the second line of defence for a selection that outlived its view.
   */
  const visibleSelected = useMemo(() => {
    const names = new Set(view.map((e) => e.name))
    const picked = new Set<string>()
    for (const name of list.selected) if (names.has(name)) picked.add(name)
    return picked
  }, [view, list.selected])

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
    if (isDir(entry.attrs)) onNavigate(tab.id, full)
    else void openPath(full).catch(() => undefined)
  }

  /** Row names only; rebuilt when the view changes, not on every keystroke. */
  const viewNames = useMemo(() => view.map((entry) => entry.name), [view])

  /**
   * Appends to the type-ahead buffer and selects the first match. The highlight is a real
   * selection, so Enter opens the match through the regular path and no filter changes.
   */
  const typeAhead = (text: string) => {
    if (!tab) return
    const buffer = typed + text
    setTyped(buffer)
    if (typeAheadTimer.current) window.clearTimeout(typeAheadTimer.current)
    typeAheadTimer.current = window.setTimeout(() => setTyped(''), 800)
    const index = matchTypeAhead(viewNames, buffer)
    if (index < 0) return
    const name = view[index].name
    update(tab.id, (d) => ({ ...d, selected: new Set([name]), anchor: name }))
    virtualizer.scrollToIndex(index, { align: 'auto' })
  }

  const clearTypeAhead = () => {
    if (typeAheadTimer.current) window.clearTimeout(typeAheadTimer.current)
    typeAheadTimer.current = null
    setTyped('')
  }

  const changeSort = (key: SortKey) => {
    const dir = pane.sort.key === key && pane.sort.dir === 'asc' ? 'desc' : 'asc'
    onSortChange({ key, dir })
  }

  /**
   * Drags the divider on a column's left edge. The governed column is the one to its
   * right, so moving right shrinks it and the divider stays under the pointer. The file
   * list may outgrow the pane and scroll horizontally; drive columns still share its width.
   */
  const startColDrag = (key: ColKey, ev: React.PointerEvent<HTMLSpanElement>) => {
    ev.preventDefault()
    ev.stopPropagation()
    onFocusPane()
    const startX = ev.clientX
    const rendered: Record<ColKey, number> = {
      size: sizeCol,
      time: timeCol,
      capacity: capacityCol,
      free: freeCol,
    }
    const stored: Record<ColKey, number> = {
      size: colSize,
      time: colTime,
      capacity: colDriveCapacity,
      free: colDriveFree,
    }
    const floors: Record<ColKey, number> = {
      size: MIN_COL_SIZE,
      time: MIN_COL_TIME,
      capacity: MIN_DRIVE_CAPACITY,
      free: MIN_DRIVE_FREE,
    }
    // Drive columns still share the pane; the file list has no such ceiling.
    const fixed =
      key === 'capacity' ? driveTypeCol + freeCol : key === 'free' ? driveTypeCol + capacityCol : 0
    const start = rendered[key] / listScale
    const min = floors[key]
    const max =
      key === 'size' || key === 'time' || width === 0
        ? MAX_COL_WIDTH
        : Math.max(min, Math.min(MAX_COL_WIDTH, (width - nameMin - fixed) / listScale))
    let next = start
    let moved = false
    const live = (): { size: number; time: number; capacity: number; free: number } => ({
      size: stored.size,
      time: stored.time,
      capacity: stored.capacity,
      free: stored.free,
      [key]: next,
    })
    const move = (e: PointerEvent) => {
      next = Math.min(max, Math.max(min, start - (e.clientX - startX) / listScale))
      const delta = Math.abs(e.clientX - startX)
      if (delta > 1) moved = true
      setDraft(live())
    }
    const end = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', end)
      window.removeEventListener('pointercancel', end)
      setDraft(null)
      if (!moved || Math.round(next) === Math.round(start)) return
      const widths = live()
      onColWidths(widths.size, widths.time, widths.capacity, widths.free)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', end)
    window.addEventListener('pointercancel', end)
  }

  /** Double-clicking a divider restores the width of the column it governs. */
  const resetCol = (key: ColKey) => {
    const defaults: Record<ColKey, number> = {
      size: DEFAULT_SETTINGS.listSizeWidth,
      time: DEFAULT_SETTINGS.listTimeWidth,
      capacity: DEFAULT_SETTINGS.driveCapacityWidth,
      free: DEFAULT_SETTINGS.driveFreeWidth,
    }
    const stored: Record<ColKey, number> = {
      size: colSize,
      time: colTime,
      capacity: colDriveCapacity,
      free: colDriveFree,
    }
    const widths = { ...stored, [key]: defaults[key] }
    onColWidths(widths.size, widths.time, widths.capacity, widths.free)
  }

  /** Opens the full-path editor; Ctrl+L uses it even at the drive overview. */
  const startAddressEdit = () => {
    addressSeq.current += 1
    setAddressBad(false)
    setAddressDraft(tab?.path ?? '')
  }

  const closeAddressEdit = () => {
    addressSeq.current += 1
    setAddressBad(false)
    setAddressDraft(null)
  }

  /**
   * Leaves only for a folder the backend confirms, so a typo keeps the current directory
   * and the editor (now marked invalid) instead of navigating into an error.
   */
  const commitAddress = () => {
    if (addressDraft === null || !tab) return
    const raw = addressDraft.trim()
    if (!raw) {
      closeAddressEdit()
      return
    }
    // Relative addresses are resolved against the current folder; `c:` lands on the root.
    const next = resolveAddress(tab.path, raw)
    if (pathKey(next) === pathKey(tab.path)) {
      closeAddressEdit()
      return
    }
    const seq = ++addressSeq.current
    void pathIsDir(next)
      .catch(() => false)
      .then((ok) => {
        if (seq !== addressSeq.current) return
        if (!ok) {
          setAddressBad(true)
          onPathInvalid(next)
          return
        }
        closeAddressEdit()
        onNavigate(tab.id, next)
      })
  }

  const selectRow = (index: number, ev: React.MouseEvent) => {
    const entry = view[index]
    if (!entry || !tab) return
    update(tab.id, (d) => {
      if (ev.shiftKey && anchorIdx >= 0) {
        const [a, b] = [Math.min(anchorIdx, index), Math.max(anchorIdx, index)]
        return { ...d, selected: new Set(view.slice(a, b + 1).map((e) => e.name)) }
      }
      if (ev.ctrlKey || ev.metaKey) {
        const next = new Set(d.selected)
        if (next.has(entry.name)) next.delete(entry.name)
        else next.add(entry.name)
        return { ...d, selected: next, anchor: entry.name }
      }
      return { ...d, selected: new Set([entry.name]), anchor: entry.name }
    })
  }

  const onKeyDown = (ev: React.KeyboardEvent) => {
    if (!tab) return
    if (matchBinding(ev, keys.properties)) {
      ev.preventDefault()
      const names = visibleSelected.size
        ? Array.from(visibleSelected)
        : view[anchorIdx]
          ? [view[anchorIdx].name]
          : []
      if (names.length) onProperties(names)
      return
    }
    if (matchBinding(ev, keys.back)) {
      ev.preventDefault()
      onHistoryMove(tab.id, -1)
      return
    }
    if (matchBinding(ev, keys.forward)) {
      ev.preventDefault()
      onHistoryMove(tab.id, 1)
      return
    }
    // Alt+Up stays a fixed alias for the parent folder; older habits outlive rebinds.
    if (matchBinding(ev, keys.up) || (ev.altKey && ev.key === 'ArrowUp')) {
      ev.preventDefault()
      onNavigate(tab.id, upTarget)
      return
    }
    if (matchBinding(ev, keys.addressBar)) {
      ev.preventDefault()
      startAddressEdit()
      return
    }
    // The drive overview holds no entries: skip selection, clipboard, and rename actions.
    if (!tab.path) return
    // An IME owns the keystrokes until it commits; shortcuts must not fire mid-composition.
    if (ev.nativeEvent.isComposing) return
    if (typed) {
      if (ev.key === 'Backspace') {
        ev.preventDefault()
        setTyped(typed.slice(0, -1))
        return
      }
      if (ev.key === 'Escape') {
        ev.preventDefault()
        clearTypeAhead()
        return
      }
      if (ev.key === 'Enter') {
        ev.preventDefault()
        const entry = view[anchorIdx]
        clearTypeAhead()
        if (entry) activate(entry)
        return
      }
      // Any other navigation ends the run, so the buffer cannot leak into the next action.
      clearTypeAhead()
    }
    if (!ev.ctrlKey && !ev.altKey && !ev.metaKey && ev.key.length === 1) {
      ev.preventDefault()
      typeAhead(ev.key)
      return
    }
    if (matchBinding(ev, keys.copy) || matchBinding(ev, keys.cut)) {
      ev.preventDefault()
      onClipboard(matchBinding(ev, keys.cut))
      return
    }
    if (matchBinding(ev, keys.selectAll)) {
      ev.preventDefault()
      update(tab.id, (d) => ({
        ...d,
        selected: new Set(view.map((e) => e.name)),
        anchor: view[0]?.name ?? '',
      }))
      return
    }
    /** Moves the selection/cursor to `next`; `extend` keeps the Shift-range anchor. */
    const moveTo = (next: number, extend: boolean) => {
      if (!view.length) return
      const target = Math.max(0, Math.min(view.length - 1, next))
      const base = anchorIdx < 0 ? target : anchorIdx
      update(tab.id, (d) => {
        if (!extend) {
          return { ...d, anchor: view[target].name, selected: new Set([view[target].name]) }
        }
        const [a, b] = [Math.min(base, target), Math.max(base, target)]
        return {
          ...d,
          anchor: view[target].name,
          selected: new Set(view.slice(a, b + 1).map((e) => e.name)),
        }
      })
      virtualizer.scrollToIndex(target, { align: 'auto' })
    }
    if (ev.key === 'ArrowDown' || ev.key === 'ArrowUp') {
      ev.preventDefault()
      if (!view.length) return
      const step = ev.key === 'ArrowDown' ? 1 : -1
      const current = anchorIdx < 0 ? (step > 0 ? -1 : 0) : anchorIdx
      moveTo(current + step, ev.shiftKey)
      return
    }
    if (
      !ev.ctrlKey &&
      !ev.altKey &&
      !ev.metaKey &&
      (ev.key === 'Home' || ev.key === 'End' || ev.key === 'PageUp' || ev.key === 'PageDown')
    ) {
      ev.preventDefault()
      const page = Math.max(
        1,
        Math.floor((scrollRef.current?.clientHeight ?? rowHeight * 10) / rowHeight) - 1,
      )
      const current = anchorIdx < 0 ? 0 : anchorIdx
      const next =
        ev.key === 'Home'
          ? 0
          : ev.key === 'End'
            ? view.length - 1
            : current + (ev.key === 'PageDown' ? page : -page)
      moveTo(next, ev.shiftKey)
      return
    }
    if (matchBinding(ev, keys.open)) {
      const entry = view[anchorIdx]
      if (entry) activate(entry)
      return
    }
    if (ev.key === 'Escape') {
      update(tab.id, (d) => ({ ...d, selected: new Set(), anchor: '' }))
    }
    if (matchBinding(ev, keys.rename)) {
      ev.preventDefault()
      if (visibleSelected.size > 1) {
        onRenameMany(Array.from(visibleSelected))
        return
      }
      const target =
        visibleSelected.size === 1 ? Array.from(visibleSelected)[0] : view[anchorIdx]?.name
      if (target) onRename(target)
      return
    }
    // Shift+Delete shares the binding: the modifier picks permanent instead of Recycle Bin.
    if (matchBinding(ev, keys.delete, { ignoreShift: true })) {
      if (!visibleSelected.size) return
      ev.preventDefault()
      onDelete(Array.from(visibleSelected), ev.shiftKey)
      return
    }
    if (matchBinding(ev, keys.refresh)) {
      ev.preventDefault()
      onRefresh()
      return
    }
    if (matchBinding(ev, keys.paste)) {
      ev.preventDefault()
      onPaste()
      return
    }
    if (matchBinding(ev, keys.contextMenu)) {
      const name = view[anchorIdx]?.name
      const row = name ? sectionRef.current?.querySelector<HTMLElement>(`[data-row="${CSS.escape(name)}"]`) : null
      if (name && row) {
        ev.preventDefault()
        const rect = row.getBoundingClientRect()
        onRowContextMenu(view[anchorIdx], {
          clientX: rect.left + 40,
          clientY: rect.top + 8,
          screenX: window.screenX + rect.left + 40,
          screenY: window.screenY + rect.top + 8,
          shiftKey: false,
        })
      } else if (tab) {
        // No row to anchor on (empty folder, or selection cleared): same as right-clicking
        // the blank area, which the shell serves with the folder-background menu.
        const rect = scrollRef.current?.getBoundingClientRect()
        if (rect) {
          ev.preventDefault()
          update(tab.id, (d) => ({ ...d, selected: new Set(), anchor: '' }))
          onBlankContextMenu({
            clientX: rect.left + 40,
            clientY: rect.top + 8,
            screenX: window.screenX + rect.left + 40,
            screenY: window.screenY + rect.top + 8,
            shiftKey: false,
          })
        }
      }
      return
    }
  }

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
      <div
        data-tabbar
        onDoubleClick={(ev) => {
          if ((ev.target as HTMLElement).closest('[data-tab]')) return
          onNewTab()
        }}
        className="typo-tab flex shrink-0 items-stretch gap-0.5 border-b border-line bg-surface-2 px-1 pt-[2px]"
        style={{ height: tabBarHeight }}
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
                className={`group flex max-w-[13.75rem] shrink-0 cursor-default items-center gap-1 rounded-t-md border border-b-0 px-1.5 leading-none ${
                  selected
                    ? 'border-line bg-surface font-medium text-fg-strong'
                    : 'border-transparent text-muted hover:bg-surface-3'
                } ${draggingTabId === id ? 'opacity-40' : ''}`}
                style={{ height: tabItemHeight }}
                title={tabState.path || t('sidebar.thisPc')}
              >
                {tabState.path ? (
                  <FileIcon name={tabState.path} dir path={tabState.path} scale={tabScale} />
                ) : (
                  <svg viewBox="0 0 16 16" className="h-[1.17em] w-[1.17em] shrink-0 text-muted-2" fill="currentColor">
                    <path d="M2 5.5C2 4.7 2.7 4 3.5 4h9c.8 0 1.5.7 1.5 1.5v5c0 .8-.7 1.5-1.5 1.5h-9C2.7 12 2 11.3 2 10.5v-5Zm2 4.3h8v.7H4v-.7Z" />
                  </svg>
                )}
                <span className="max-w-[9.375rem] truncate">
                  {tabState.path
                    ? (tabState.path.split('\\').filter(Boolean).pop() ?? tabState.path)
                    : t('sidebar.thisPc')}
                </span>
                <button
                  type="button"
                  onPointerDown={(ev) => ev.stopPropagation()}
                  onClick={(ev) => {
                    ev.stopPropagation()
                    onCloseTab(id)
                  }}
                  className="grid h-[1.33em] w-[1.33em] shrink-0 place-items-center rounded text-muted-2 hover:bg-surface-3 hover:text-fg"
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
          className="grid h-[1.67em] w-[1.67em] shrink-0 place-items-center self-center rounded text-muted hover:bg-surface-3 hover:text-fg"
          title={t('tab.new')}
        >
          <svg viewBox="0 0 16 16" className="h-[1.17em] w-[1.17em]" fill="none" stroke="currentColor" strokeWidth="1.6">
            <path d="M8 3.5v9M3.5 8h9" strokeLinecap="round" />
          </svg>
        </button>
      </div>

      <div
        className="typo-nav flex shrink-0 items-center gap-0.5 border-b border-line bg-surface px-1"
        style={{ height: navBarHeight }}
      >
        <button
          type="button"
          title={t('nav.back')}
          onClick={() => tab && onHistoryMove(tab.id, -1)}
          disabled={!tab || tab.histIdx <= 0}
          style={navButtonStyle}
          className="grid place-items-center rounded text-muted hover:bg-surface-3 disabled:opacity-30"
        >
          <svg viewBox="0 0 16 16" className="h-[1.17em] w-[1.17em]" fill="none" stroke="currentColor" strokeWidth="1.6">
            <path d="M10 3 5.5 8 10 13" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        </button>
        <button
          type="button"
          title={t('nav.forward')}
          onClick={() => tab && onHistoryMove(tab.id, 1)}
          disabled={!tab || tab.histIdx >= tab.history.length - 1}
          style={navButtonStyle}
          className="grid place-items-center rounded text-muted hover:bg-surface-3 disabled:opacity-30"
        >
          <svg viewBox="0 0 16 16" className="h-[1.17em] w-[1.17em]" fill="none" stroke="currentColor" strokeWidth="1.6">
            <path d="M6 3l4.5 5L6 13" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        </button>
        <button
          type="button"
          title={t('nav.up')}
          onClick={() => tab && onNavigate(tab.id, upTarget)}
          disabled={!tab || upTarget === tab.path}
          style={navButtonStyle}
          className="grid place-items-center rounded text-muted hover:bg-surface-3 disabled:opacity-30"
        >
          <svg viewBox="0 0 16 16" className="h-[1.17em] w-[1.17em]" fill="none" stroke="currentColor" strokeWidth="1.6">
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
          style={navFieldStyle}
          className="ml-1 flex shrink-0 items-center gap-0.5 rounded border border-line bg-surface-2 px-1.5 leading-none text-fg hover:bg-surface-3"
        >
          <span className="font-medium">
            {location ? location.name : root ? t('sidebar.thisPc') : driveRoot(tab?.path)}
          </span>
          <svg viewBox="0 0 16 16" className="h-[1em] w-[1em] text-muted-2" fill="none" stroke="currentColor" strokeWidth="1.6">
            <path d="m4 6.5 4 4 4-4" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        </button>

        {addressDraft !== null ? (
          <input
            ref={addressRef}
            data-address-input
            value={addressDraft}
            spellCheck={false}
            onChange={(ev) => {
              setAddressDraft(ev.target.value)
              setAddressBad(false)
            }}
            onBlur={closeAddressEdit}
            onKeyDown={(ev) => {
              if (ev.key === 'Enter') {
                ev.preventDefault()
                commitAddress()
              } else if (ev.key === 'Escape') {
                ev.preventDefault()
                closeAddressEdit()
              }
            }}
            title={addressBad ? t('address.invalidPath') : undefined}
            style={navFieldStyle}
            className={`mx-0.5 min-w-0 flex-1 rounded border bg-surface px-1.5 outline-none ${
              addressBad ? 'border-red-400 text-red-500' : 'border-blue-400'
            }`}
          />
        ) : (
        <div
          style={navFieldStyle}
          className="mx-0.5 flex min-w-0 flex-1 items-center rounded border border-line bg-surface-2 leading-none"
          // Blank space opens the full-path editor; crumbs and the copy button keep their clicks.
          onClick={(ev) => {
            if (!tab?.path || (ev.target as HTMLElement).closest('button')) return
            startAddressEdit()
          }}
        >
          <div className="no-bar flex h-full min-w-0 flex-1 items-center overflow-x-auto overflow-y-hidden px-1">
            {root && (
              <span className="shrink-0 px-1 py-[0.25em] font-medium leading-none text-fg-strong">
                {t('sidebar.thisPc')}
              </span>
            )}
            {parts.map((part, i) => (
              <div key={part.path} className="flex shrink-0 items-center">
                {i > 0 && <span className="px-0.5 text-muted-2">›</span>}
                <button
                  type="button"
                  data-crumb={part.path}
                  // A crumb jumps straight to that folder, like Explorer's breadcrumbs.
                  onClick={() => tab && pathKey(part.path) !== pathKey(tab.path) && onNavigate(tab.id, part.path)}
                  title={part.path}
                  className={`max-w-[10rem] truncate rounded px-1 py-[0.25em] leading-none hover:bg-surface-3 ${
                    i === parts.length - 1 ? 'font-medium text-fg-strong' : 'text-muted'
                  }`}
                >
                  {part.label}
                </button>
              </div>
            ))}
          </div>
          {tab?.path && (
            <button
              type="button"
              data-copy-path
              onClick={() => onCopyPath(tab.path)}
              title={t('address.copyPath')}
              className="grid h-full w-[1.67em] shrink-0 place-items-center border-l border-line text-muted-2 hover:bg-surface-3 hover:text-fg"
            >
              <svg viewBox="0 0 16 16" className="h-[1em] w-[1em]" fill="none" stroke="currentColor" strokeWidth="1.4">
                <rect x="3.5" y="3.5" width="7" height="9" rx="1.2" />
                <path d="M6 3.5v-.7a1 1 0 0 1 1-1h4.3a1 1 0 0 1 1 1v7.4a1 1 0 0 1-1 1H9.5" />
              </svg>
            </button>
          )}
        </div>
        )}

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
        <div className="shrink-0 overflow-hidden" style={{ height: headerHeight, marginRight: gutter }}>
          <div
            ref={headerRef}
            className="typo-list grid h-full items-center border-b border-line bg-surface-2 pr-1.5 text-muted"
            style={{ gridTemplateColumns: grid, minWidth: listMinWidth }}
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
          if (e.target === e.currentTarget && tab) {
            update(tab.id, (d) => ({ ...d, selected: new Set(), anchor: '' }))
          }
        }}
        onContextMenu={(e) => {
          e.preventDefault()
          if (e.target !== e.currentTarget) return
          if (tab) update(tab.id, (d) => ({ ...d, selected: new Set(), anchor: '' }))
          onBlankContextMenu(pointOf(e))
        }}
        data-drop-dir={tab?.path || undefined}
        className="typo-list relative min-h-0 flex-1 overflow-auto outline-none"
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
          <div className="py-0.5">
            {drives.map((drive) => {
              const sized = drive.available && drive.total > 0
              const used = sized ? Math.max(0, drive.total - drive.free) : 0
              const usedRatio = sized ? Math.min(1, used / drive.total) : 0
              const low = sized && drive.free / drive.total < 0.1
              return (
                <div
                  key={`${drive.kind}:${drive.name}`}
                  data-drive={drive.root}
                  onDoubleClick={() =>
                    tab &&
                    (isFsPath(drive.root)
                      ? onNavigate(tab.id, drive.root)
                      : void openPath(drive.root).catch(() => undefined))
                  }
                  onContextMenu={(ev) => {
                    ev.preventDefault()
                    onDriveRowMenu(drive, pointOf(ev))
                  }}
                  title={drive.available ? drive.root : `${drive.root} · ${t('drive.unavailable')}`}
                  style={{ height: rowHeight, gridTemplateColumns: driveGrid }}
                  className={`grid cursor-default items-center gap-2 border-b border-line/60 px-1.5 hover:bg-surface-3 ${
                    drive.available ? '' : 'text-muted-2'
                  }`}
                >
                  <div className="flex min-w-0 items-center gap-1.5">
                    <FileIcon name={drive.name} dir path={drive.root} scale={listScale} />
                    <span className={`truncate font-medium ${drive.available ? 'text-fg-strong' : 'text-muted'}`}>
                      {drive.name}
                    </span>
                  </div>
                  {showDriveLabel && (
                    <span className="truncate text-[0.88em] text-muted">
                      {drive.label || (drive.kind === 'location' ? '' : '—')}
                    </span>
                  )}
                  <span className="truncate text-[0.88em] text-muted-2">{driveKindLabel(t, drive.kind)}</span>
                  {showDriveCapacity && (
                    <>
                      <div className="flex min-w-0 flex-col gap-0.5">
                        <span className="truncate text-right text-[0.8em] text-muted">
                          {sized ? `${formatSize(used, false)} / ${formatSize(drive.total, false)}` : '—'}
                        </span>
                        {sized && (
                          <div className="h-0.5 overflow-hidden rounded-full bg-surface-3">
                            <div
                              className={`h-full rounded-full ${low ? 'bg-red-500' : 'bg-blue-500'}`}
                              style={{ width: `${Math.round(usedRatio * 100)}%` }}
                            />
                          </div>
                        )}
                      </div>
                      <span
                        className={`truncate text-right text-[0.8em] ${low ? 'text-red-500' : 'text-muted'}`}
                      >
                        {sized ? formatSize(drive.free, false) : '—'}
                      </span>
                    </>
                  )}
                </div>
              )
            })}
          </div>
        )}

        {!root && <div style={{ height: virtualizer.getTotalSize(), position: 'relative', minWidth: listMinWidth }}>
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
                data-drop-dir={dir ? joinPath(tab.path, entry.name) : undefined}
                onPointerDown={(e) => {
                  if (e.button !== 0) return
                  selectRow(vi.index, e)
                  onRowDragStart(entry, e)
                }}
                onContextMenu={(e) => {
                  e.preventDefault()
                  if (!list.selected.has(entry.name)) selectRow(vi.index, e)
                  onRowContextMenu(entry, pointOf(e))
                }}
                onDoubleClick={() => activate(entry)}
                className={`absolute left-0 top-0 grid w-full items-center border-b border-line/60 pr-1.5 ${
                  isSelected ? 'bg-sel text-fg-strong' : 'hover:bg-surface-3'
                }`}
                style={{ height: rowHeight, transform: `translateY(${vi.start}px)`, gridTemplateColumns: grid }}
              >
                <div className="flex min-w-0 items-center gap-1.5 pl-1.5">
                  <span className="relative shrink-0">
                    <FileIcon name={entry.name} dir={dir} path={joinPath(tab.path, entry.name)} scale={listScale} />
                    <GitRepoBadge name={entry.name} dir={dir} badges={gitBadges} t={t} />
                  </span>
                  <span className={`truncate ${dir ? 'font-medium text-fg-strong' : ''}`}>{entry.name}</span>
                  <GitBadge name={entry.name} dir={dir} badges={gitBadges} />
                </div>
                {showSize && (
                  <div className="tabular whitespace-nowrap pr-1.5 text-right text-[0.88em] text-muted">
                    {formatSize(entry.size, dir)}
                  </div>
                )}
                {showTime && (
                  <div className="tabular whitespace-nowrap pl-1.5 text-[0.88em] text-muted">
                    {formatTime(entry.mtime)}
                  </div>
                )}
              </div>
            )
          })}
        </div>}

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

      <div
        data-status
        className="flex h-6 shrink-0 items-center justify-between border-t border-line bg-surface-2 px-2 text-[0.65625rem] text-muted"
      >
        <div className="flex min-w-0 items-center gap-2">
          {git?.root && <GitChip git={git} t={t} />}
          <span>{root ? t('list.drives', { count: drives.length }) : t('list.items', { count: view.length })}</span>
          {list.filter && list.entries.length !== view.length && (
            <span>{t('list.total', { count: list.entries.length })}</span>
          )}
          {visibleSelected.size > 0 && (
            <span>
              {t('list.selected', { count: visibleSelected.size })}
              {selectedSize > 0 && ` · ${formatSize(selectedSize, false)}`}
            </span>
          )}
          {list.loading && list.entries.length > 0 && (
            <span>{t('list.scanning', { count: list.scanned })}</span>
          )}
        </div>
        <span className="max-w-[45%] truncate font-mono text-[0.59375rem] text-muted-2">{tab?.path}</span>
      </div>

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

function GitBranchIcon({ className }: { className: string }) {
  return (
    <svg
      viewBox="0 0 16 16"
      className={className}
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
    >
      <circle cx="4.5" cy="3.5" r="1.75" />
      <circle cx="4.5" cy="12.5" r="1.75" />
      <circle cx="11.5" cy="6" r="1.75" />
      <path d="M4.5 5.25v5.5M11.5 7.75c0 2.5-2.1 3.6-4.6 4" />
    </svg>
  )
}

/** Branch, uncommitted state and upstream distance; visible even when the tree is clean. */
function GitChip({ git, t }: { git: GitStatus; t: Translate }) {
  const title = [
    git.branch || t('git.detached'),
    git.dirty ? t('git.dirty') : t('git.clean'),
    git.upstream ? '' : t('git.upstreamMissing'),
    git.ahead > 0 ? t('git.ahead', { count: git.ahead }) : '',
    git.behind > 0 ? t('git.behind', { count: git.behind }) : '',
  ]
    .filter(Boolean)
    .join(' · ')
  return (
    <span className="flex min-w-0 max-w-[11.25rem] shrink-0 items-center gap-1" title={title}>
      <GitBranchIcon className="h-3 w-3 shrink-0 text-blue-500" />
      <span className="truncate font-mono text-[0.625rem] text-fg-strong">{git.branch || t('git.detached')}</span>
      {!git.upstream && <span className="shrink-0 text-muted-2">{t('git.upstreamMissingShort')}</span>}
      {git.dirty && <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-amber-500" />}
      {git.ahead > 0 && <span className="shrink-0 text-emerald-500">↑{git.ahead}</span>}
      {git.behind > 0 && <span className="shrink-0 text-red-500">↓{git.behind}</span>}
    </span>
  )
}

/** Corner badge on a repository folder icon: conflict, pending, unpushed, or clean. */
function GitRepoBadge({
  name,
  dir,
  badges,
  t,
}: {
  name: string
  dir: boolean
  badges: GitBadges
  t: Translate
}) {
  if (!dir) return null
  const repo = badges.repos.get(name.toLowerCase())
  if (!repo) return null
  if (!repo.checked) {
    return <GitBranchIcon className="absolute -bottom-0.5 -right-1 h-3 w-3 text-blue-500 drop-shadow" />
  }
  const title = [
    repo.conflict ? t('git.conflict') : repo.dirty ? t('git.dirty') : t('git.clean'),
    repo.upstream ? '' : t('git.upstreamMissing'),
    repo.ahead > 0 ? t('git.ahead', { count: repo.ahead }) : '',
    repo.behind > 0 ? t('git.behind', { count: repo.behind }) : '',
  ]
    .filter(Boolean)
    .join(' · ')
  const tone = repo.conflict
    ? 'bg-red-500'
    : repo.dirty
      ? 'bg-amber-500'
      : repo.ahead > 0
        ? 'bg-blue-500'
        : repo.behind > 0
          ? 'bg-violet-500'
          : 'bg-emerald-500'
  return (
    <span
      title={title}
      className={`absolute -bottom-0.5 -right-1 grid h-3 w-3 place-items-center rounded-full text-white ring-1 ring-surface ${tone}`}
    >
      {repo.conflict ? (
        <span className="text-[0.5625rem] font-bold leading-none">!</span>
      ) : repo.dirty ? (
        <svg viewBox="0 0 16 16" className="h-2 w-2" fill="currentColor" aria-hidden>
          <path d="M4 12.4l.9-3.3 5.9-5.9a1.3 1.3 0 0 1 1.8 0l.2.2a1.3 1.3 0 0 1 0 1.8L6.9 11.1 2.9 12.4Z" />
        </svg>
      ) : repo.ahead > 0 ? (
        <ArrowGlyph up />
      ) : repo.behind > 0 ? (
        <ArrowGlyph />
      ) : (
        <svg
          viewBox="0 0 16 16"
          className="h-2 w-2"
          fill="none"
          stroke="currentColor"
          strokeWidth="2.4"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden
        >
          <path d="M3.4 8.6 6.4 11.6 12.6 4.8" />
        </svg>
      )}
    </span>
  )
}

function ArrowGlyph({ up = false }: { up?: boolean }) {
  return (
    <svg
      viewBox="0 0 16 16"
      className="h-2 w-2"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d={up ? 'M8 12.5V4M4.6 7.4 8 4l3.4 3.4' : 'M8 3.5V12M4.6 8.6 8 12l3.4-3.4'} />
    </svg>
  )
}

/**
 * Porcelain letter for a changed file, and a dot when a folder holds changes below it.
 * Repository folders carry their state on the icon badge instead.
 */
function GitBadge({
  name,
  dir,
  badges,
}: {
  name: string
  dir: boolean
  badges: GitBadges
}) {
  const key = name.toLowerCase()
  if (dir && badges.repos.has(key)) return null
  const code = badges.changed.get(key)
  if (code) {
    return (
      <span title={code} className={`shrink-0 text-[0.625rem] font-semibold ${gitTone(code)}`}>
        {code}
      </span>
    )
  }
  if (dir && badges.dirtyDirs.has(key)) {
    return <span title="git" className="h-1.5 w-1.5 shrink-0 rounded-full bg-blue-500/70" />
  }
  return null
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
      className={`flex h-full w-full min-w-0 items-center gap-1 px-1.5 text-[0.84em] font-medium tracking-wide text-muted hover:text-fg ${
        align === 'right' ? 'justify-end' : 'justify-start'
      }`}
    >
      <span className="truncate">{label}</span>
      <span className={`shrink-0 text-[0.64em] ${active ? 'text-blue-500' : 'text-transparent'}`}>
        {sort.dir === 'asc' ? '▲' : '▼'}
      </span>
    </button>
  )
}

/** Hit area on a column's left edge: drag resizes it, double-click restores the default. */
function ColDivider({
  label,
  onPointerDown,
  onDoubleClick,
}: {
  label: string
  onPointerDown: (ev: React.PointerEvent<HTMLSpanElement>) => void
  onDoubleClick: () => void
}) {
  return (
    <span
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      onPointerDown={onPointerDown}
      onDoubleClick={onDoubleClick}
      className="absolute inset-y-0 left-0 z-10 w-1.5 -translate-x-1/2 cursor-col-resize touch-none select-none hover:bg-blue-400/60 active:bg-blue-500"
    />
  )
}

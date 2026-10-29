import type { Lang, Theme } from './types'
import type { Node, PaneState, TabState, Workspace } from './workspace'
import type { ShortcutOverride } from './shortcuts'

export type QuickPlace = { name: string; path: string }

/** Program used for extensions the built-in previewer cannot render. */
export type ExternalPreviewRule = { ext: string; command: string }

/** Backend diagnostic log verbosity. */
export type LogLevel = 'error' | 'warn' | 'info' | 'debug'

export type Settings = {
  language: Lang
  theme: Theme
  previewOpen: boolean
  showHidden: boolean
  useEverything: boolean
  sidebarOpen: boolean
  /** Pixel width of the sidebar while open; drag-resizable. */
  sidebarWidth: number
  /** Pixel width of the preview panel while open; drag-resizable. */
  previewWidth: number
  customPlaces: QuickPlace[]
  hiddenPlaces: string[]
  externalPreview: ExternalPreviewRule[]
  shortcuts: ShortcutOverride[]
  gpuAccel: boolean
  reclaimMemory: boolean
  updateIntervalHours: number
  /** System notification when a long operation finishes while the window is unfocused. */
  notifyOps: boolean
  /** Diagnostic log verbosity; the backend log follows it immediately. */
  logLevel: LogLevel
  /** Single UI font family; the frontend appends fallbacks. */
  fontFamily: string
  /** Base UI font size in px (12–20); drives the whole rem scale. */
  fontSize: number
  /** Tab-strip font family; empty inherits `fontFamily`. */
  tabFontFamily: string
  /** Tab-strip text size in px (10–20); drives the strip height and row icons. */
  tabFontSize: number
  /** File-list font family; empty inherits `fontFamily`. */
  listFontFamily: string
  /** File-list text size in px (10–20); drives rows, columns and icons. */
  listFontSize: number
  /** Navigation-bar font family; empty inherits `fontFamily`. */
  navFontFamily: string
  /** Navigation-bar text size in px (10–20); drives its controls. */
  navFontSize: number
  /** File-list "size" column visibility. */
  listColSize: boolean
  /** File-list "type" column visibility. */
  listColType: boolean
  /** File-list "modified" column visibility. */
  listColTime: boolean
  /**
   * File-list name column width in px at the default font size; `0` fills the pane.
   * A frozen width leaves the rest of the row blank for folder-menu clicks.
   */
  listNameWidth: number
  /** File-list "size" column width in px at the default font size; drag-resizable. */
  listSizeWidth: number
  /** File-list "type" column width in px at the default font size; drag-resizable. */
  listTypeWidth: number
  /** File-list "modified" column width in px at the default font size; drag-resizable. */
  listTimeWidth: number
  /** Drive overview "capacity" column width in px at the default font size; drag-resizable. */
  driveCapacityWidth: number
  /** Drive overview "free" column width in px at the default font size; drag-resizable. */
  driveFreeWidth: number
  /** Undo and redo history depth (1–100). */
  undoLimit: number
  /** Largest file the preview reads, in MB (1–4096). */
  previewMaxFileMb: number
  /** Largest width or height a rendered preview may use (256–16384). */
  previewMaxDimension: number
  /** How much of a text file the preview shows, in KB (1–1024). */
  previewTextKb: number
  /** Closed tabs remembered for Ctrl+Shift+T (1–50). */
  closedTabLimit: number
  /** Restore the previous tab set on launch. */
  restoreTabsOnStart: boolean
  /** Middle-click / Ctrl+click a folder opens it in a background tab. */
  newTabOnMiddleClick: boolean
  /** Duplicate-scan worker threads; 0 = auto. */
  dupConcurrency: number
  /** Duplicate-scan read cap in MB/s; 0 = unlimited. */
  dupMaxMbPerSec: number
  /** Directory names the duplicate scan never descends into. */
  dupExclude: string[]
  /** Global hotkey switch; the combination itself is in `globalHotkey`. */
  hotkeyEnabled: boolean
  /** Canonical combination such as `ctrl+alt+f`. */
  globalHotkey: string
  /** "Open with FlowDir" entries in Explorer's context menus. */
  contextMenu: boolean
  /** FlowDir appears under "Open with" for folders and drives; never the default verb. */
  associateFolders: boolean
  /** Start with Windows through the HKCU Run key. */
  autostart: boolean
  /** With `autostart`, start hidden in the tray. */
  autostartMinimized: boolean
}

export const DEFAULT_SETTINGS: Settings = {
  language: 'zh-CN',
  theme: 'system',
  previewOpen: true,
  showHidden: false,
  useEverything: true,
  sidebarOpen: true,
  sidebarWidth: 176,
  previewWidth: 320,
  customPlaces: [],
  hiddenPlaces: [],
  externalPreview: [],
  shortcuts: [],
  gpuAccel: true,
  reclaimMemory: false,
  updateIntervalHours: 24,
  notifyOps: true,
  logLevel: 'info',
  fontFamily: 'Segoe UI Variable Text',
  fontSize: 13,
  tabFontFamily: '',
  tabFontSize: 12,
  listFontFamily: '',
  listFontSize: 13,
  navFontFamily: '',
  navFontSize: 12,
  listColSize: true,
  listColType: true,
  listColTime: true,
  listNameWidth: 0,
  listSizeWidth: 96,
  listTypeWidth: 88,
  listTimeWidth: 142,
  driveCapacityWidth: 150,
  driveFreeWidth: 84,
  undoLimit: 10,
  previewMaxFileMb: 64,
  previewMaxDimension: 4096,
  previewTextKb: 8,
  closedTabLimit: 20,
  restoreTabsOnStart: true,
  newTabOnMiddleClick: true,
  dupConcurrency: 0,
  dupMaxMbPerSec: 0,
  dupExclude: [
    'node_modules',
    '.git',
    '$Recycle.Bin',
    'System Volume Information',
    'Windows',
    'Program Files',
    'Program Files (x86)',
  ],
  hotkeyEnabled: false,
  globalHotkey: 'ctrl+alt+f',
  contextMenu: false,
  associateFolders: false,
  autostart: false,
  autostartMinimized: true,
}

type Snapshot = {
  root: Node
  panes: Record<string, PaneState>
  tabs: Record<string, TabState>
  activePaneId: string
}

export function toSnapshot(ws: Workspace): Snapshot {
  return { root: ws.root, panes: ws.panes, tabs: ws.tabs, activePaneId: ws.activePaneId }
}

function isNode(value: unknown, panes: Record<string, unknown>): value is Node {
  if (!value || typeof value !== 'object') return false
  const node = value as Record<string, unknown>
  if (node.t === 'pane') return typeof node.id === 'string' && node.id in panes
  if (node.t === 'split') {
    return (
      typeof node.id === 'string' &&
      (node.dir === 'row' || node.dir === 'col') &&
      typeof node.ratio === 'number' &&
      isNode(node.a, panes) &&
      isNode(node.b, panes)
    )
  }
  return false
}

/** Validates a persisted workspace; returns null when the shape is unusable. */
export function fromSnapshot(value: unknown): Workspace | null {
  if (!value || typeof value !== 'object') return null
  const snapshot = value as Record<string, unknown>
  const panes = snapshot.panes
  const tabs = snapshot.tabs
  if (!panes || typeof panes !== 'object' || !tabs || typeof tabs !== 'object') return null
  if (!isNode(snapshot.root, panes as Record<string, unknown>)) return null

  const paneMap = panes as Record<string, PaneState>
  const tabMap = tabs as Record<string, TabState>
  for (const pane of Object.values(paneMap)) {
    if (!Array.isArray(pane.tabIds) || !pane.tabIds.length) return null
    if (!pane.tabIds.every((id) => id in tabMap)) return null
    if (!(pane.activeTabId in tabMap)) return null
    if (!pane.sort || typeof pane.sort.key !== 'string') return null
  }
  for (const tab of Object.values(tabMap)) {
    // An empty path is the drive overview tab and must survive a restart.
    if (typeof tab.path !== 'string') return null
    if (!Array.isArray(tab.history) || typeof tab.histIdx !== 'number') return null
  }
  const activePaneId =
    typeof snapshot.activePaneId === 'string' && snapshot.activePaneId in paneMap
      ? snapshot.activePaneId
      : Object.keys(paneMap)[0]

  return {
    root: snapshot.root,
    panes: paneMap,
    tabs: tabMap,
    activePaneId,
  }
}

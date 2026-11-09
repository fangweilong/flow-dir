import type { Lang, Theme } from './types'
import type { IconThemeId } from './iconThemes/catalog'
import type { Node, PaneState, TabState, Workspace } from './workspace'
import type { ShortcutOverride } from './shortcuts'

export type QuickPlace = { name: string; path: string }

/** Program used for extensions the built-in previewer cannot render. */
export type ExternalPreviewRule = { ext: string; command: string }

/**
 * Paint rule for the file lists: `pattern` is a case-insensitive glob (`*`, `?`) or a
 * plain substring when it has no wildcard. Empty colors inherit the theme.
 */
export type ColorRule = {
  pattern: string
  /** `#rrggbb`, or empty to keep the theme's text color. */
  foreground: string
  /** `#rrggbb`, or empty for no highlight background. */
  background: string
  /** Also paint directories that match; off by default so files stand out. */
  folders: boolean
}

/** Backend diagnostic log verbosity. */
export type LogLevel = 'error' | 'warn' | 'info' | 'debug'

/**
 * One WebDAV server. `id` is the stable address component (`webdav://<id>/…`) and the
 * credential-manager key; editing the display name must not change it.
 */
export type WebdavConnection = {
  id: string
  name: string
  /** Absolute `http(s)://host[:port]/base` URL without a trailing slash. */
  url: string
  username: string
  /** Folder under `url` used as the virtual root; no leading/trailing slash. */
  root: string
  /** Accept self-signed or expired certificates for this server only. */
  insecureTls: boolean
}

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
  /** Named WebDAV servers; passwords live in Windows Credential Manager. */
  webdav: WebdavConnection[]
  externalPreview: ExternalPreviewRule[]
  colorRules: ColorRule[]
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
  /** File and drive list icon size in px (14–32). */
  iconSize: number
  /** Icon style for file and folder rows: Windows shell icons or a bundled theme. */
  iconTheme: IconThemeId
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
  /** Delete/move/rename on these paths (or their ancestors) is refused outright. */
  protectBlock: string[]
  /** Delete/move/rename on these paths requires a typed confirmation. */
  protectConfirm: string[]
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
  webdav: [],
  externalPreview: [],
  colorRules: [],
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
  iconSize: 20,
  iconTheme: 'system',
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
  // Must match the backend default (`DEFAULT_PREVIEW_TEXT_KB`); the old 8 KB value only
  // applied before settings arrived, then silently changed the effective size.
  previewTextKb: 256,
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
  // The backend expands the %VAR% placeholders on load; these values only cover the
  // window between the initial render and `settings_load`.
  protectBlock: [
    '%SystemRoot%',
    '%ProgramFiles%',
    '%ProgramFiles(x86)%',
    '%ProgramData%',
    '%USERPROFILE%',
    '%USERPROFILE%\\AppData',
    '%APPDATA%',
    '%LOCALAPPDATA%',
  ],
  protectConfirm: [],
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
  // `in` also accepts inherited keys (`constructor`, `toString`, `__proto__`), which a
  // hand-edited layout.json can use to pass validation without a matching own record.
  if (node.t === 'pane') return typeof node.id === 'string' && Object.hasOwn(panes, node.id)
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

  // A pane record the split tree does not reference is invisible to the user but would
  // still be accepted as the active pane (and drive keyboard/preview/refresh). Drop it
  // before validating, so junk in an unused record cannot block the real layout either.
  const livePaneIds = new Set<string>()
  const collectPanes = (node: Node) => {
    if (node.t === 'pane') {
      livePaneIds.add(node.id)
      return
    }
    collectPanes(node.a)
    collectPanes(node.b)
  }
  collectPanes(snapshot.root as Node)
  const paneMap = Object.fromEntries(
    Object.entries(panes as Record<string, PaneState>).filter(([id]) => livePaneIds.has(id)),
  )
  const tabMap = tabs as Record<string, TabState>
  for (const pane of Object.values(paneMap)) {
    // `layout.json` is user-editable data; a `null` member must reject the snapshot, not
    // throw out of the bootstrap and leave the window empty with no recovery path.
    if (!pane || typeof pane !== 'object') return null
    if (!Array.isArray(pane.tabIds) || !pane.tabIds.length) return null
    if (!pane.tabIds.every((id) => Object.hasOwn(tabMap, id))) return null
    if (!Object.hasOwn(tabMap, pane.activeTabId)) return null
    // The active tab must belong to this pane, otherwise the filter below can drop the
    // record and leave the pane with no active tab.
    if (!pane.tabIds.includes(pane.activeTabId)) return null
    if (!pane.sort || typeof pane.sort.key !== 'string') return null
    // `layout.json` is hand-editable; an unknown view would leave the pane with no body.
    if (
      pane.view !== undefined &&
      !['details', 'list', 'icons', 'thumbs'].includes(pane.view as string)
    ) {
      delete pane.view
    }
  }
  for (const tab of Object.values(tabMap)) {
    // An empty path is the drive overview tab and must survive a restart.
    if (!tab || typeof tab !== 'object') return null
    if (typeof tab.path !== 'string') return null
    if (!Array.isArray(tab.history) || typeof tab.histIdx !== 'number') return null
    if (!tab.history.every((entry) => typeof entry === 'string')) return null
  }
  // A tab id shared by two panes would let closing it in one pane remove the record the
  // other pane still references.
  const seenTabIds = new Set<string>()
  for (const pane of Object.values(paneMap)) {
    for (const id of pane.tabIds) {
      if (seenTabIds.has(id)) return null
      seenTabIds.add(id)
    }
  }
  // Older builds could leave tab records no pane points at; restoring them would show
  // closed folders in the compare pickers forever and keep growing the file.
  const referenced = new Set(Object.values(paneMap).flatMap((pane) => pane.tabIds))
  const liveTabs = Object.fromEntries(
    Object.entries(tabMap).filter(([id]) => referenced.has(id)),
  )
  const activePaneId =
    typeof snapshot.activePaneId === 'string' && Object.hasOwn(paneMap, snapshot.activePaneId)
      ? snapshot.activePaneId
      : Object.keys(paneMap)[0]

  return {
    root: snapshot.root,
    panes: paneMap,
    tabs: liveTabs,
    activePaneId,
  }
}

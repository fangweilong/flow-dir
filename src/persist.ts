import type { Lang, Theme } from './types'
import type { Node, PaneState, TabState, Workspace } from './workspace'
import type { ShortcutOverride } from './shortcuts'

export type QuickPlace = { name: string; path: string }

/** Program used for extensions the built-in previewer cannot render. */
export type ExternalPreviewRule = { ext: string; command: string }

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
  updateIntervalHours: number
  /** Single UI font family; the frontend appends fallbacks. */
  fontFamily: string
  /** Base UI font size in px (12–20); drives the whole rem scale. */
  fontSize: number
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
  updateIntervalHours: 24,
  fontFamily: 'Segoe UI Variable Text',
  fontSize: 13,
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

import type { SortState } from './types'

export type TabState = { id: string; path: string; history: string[]; histIdx: number }
export type PaneState = {
  id: string
  tabIds: string[]
  activeTabId: string
  sort: SortState
  hidden: boolean
}
export type Node =
  | { t: 'split'; id: string; dir: 'row' | 'col'; ratio: number; a: Node; b: Node }
  | { t: 'pane'; id: string }
export type Workspace = {
  root: Node
  panes: Record<string, PaneState>
  tabs: Record<string, TabState>
  activePaneId: string
}

export type DropMode = 'center' | 'left' | 'right' | 'top' | 'bottom'

let seq = 0
export const uid = (prefix: string) => `${prefix}${++seq}`

/**
 * Advances the id counter past every numeric suffix already present in a restored
 * workspace. Without this, ids minted after a restart reuse `p1`/`t1` and collide with
 * the restored panes and tabs.
 */
export function adoptIds(ws: Workspace) {
  let max = 0
  const scan = (node: Node) => {
    if (node.t === 'split') {
      scan(node.a)
      scan(node.b)
      return
    }
    for (const id of [node.id, ...(ws.panes[node.id]?.tabIds ?? [])]) {
      const match = /\d+$/.exec(id)
      if (match) max = Math.max(max, Number(match[0]))
    }
  }
  scan(ws.root)
  max = Math.max(max, Object.keys(ws.panes).length, Object.keys(ws.tabs).length)
  seq = Math.max(seq, max)
}

export const DEFAULT_SORT: SortState = { key: 'name', dir: 'asc' }

export function paneIds(node: Node): string[] {
  return node.t === 'pane' ? [node.id] : [...paneIds(node.a), ...paneIds(node.b)]
}

export function replacePane(node: Node, paneId: string, next: Node): Node {
  if (node.t === 'pane') return node.id === paneId ? next : node
  return { ...node, a: replacePane(node.a, paneId, next), b: replacePane(node.b, paneId, next) }
}

export function removePane(node: Node, paneId: string): Node | null {
  if (node.t === 'pane') return node.id === paneId ? null : node
  const a = removePane(node.a, paneId)
  const b = removePane(node.b, paneId)
  if (!a) return b
  if (!b) return a
  return { ...node, a, b }
}

export function setRatio(node: Node, splitId: string, ratio: number): Node {
  if (node.t === 'pane') return node
  if (node.id === splitId) return { ...node, ratio }
  return { ...node, a: setRatio(node.a, splitId, ratio), b: setRatio(node.b, splitId, ratio) }
}

export function splitPane(
  node: Node,
  paneId: string,
  dir: 'row' | 'col',
  side: 'before' | 'after',
  newPaneId: string,
): Node {
  const split: Node = {
    t: 'split',
    id: uid('s'),
    dir,
    ratio: 0.5,
    a: { t: 'pane', id: side === 'before' ? newPaneId : paneId },
    b: { t: 'pane', id: side === 'before' ? paneId : newPaneId },
  }
  return replacePane(node, paneId, split)
}

export function buildPreset(ids: string[], kind: 1 | 2 | 3 | 4, orient: 'h' | 'v'): Node {
  const pane = (id: string): Node => ({ t: 'pane', id })
  const split = (dir: 'row' | 'col', a: Node, b: Node): Node => ({
    t: 'split',
    id: uid('s'),
    dir,
    ratio: 0.5,
    a,
    b,
  })
  if (kind === 1) return pane(ids[0])
  if (kind === 2) return split(orient === 'h' ? 'row' : 'col', pane(ids[0]), pane(ids[1]))
  if (kind === 3) {
    return split('row', pane(ids[0]), split('col', pane(ids[1]), pane(ids[2])))
  }
  return split('row', split('col', pane(ids[0]), pane(ids[1])), split('col', pane(ids[2]), pane(ids[3])))
}

export function createWorkspace(home: string, hidden = false): Workspace {
  const tabId = uid('t')
  const paneId = uid('p')
  return {
    root: { t: 'pane', id: paneId },
    panes: {
      [paneId]: { id: paneId, tabIds: [tabId], activeTabId: tabId, sort: DEFAULT_SORT, hidden },
    },
    tabs: { [tabId]: { id: tabId, path: home, history: [home], histIdx: 0 } },
    activePaneId: paneId,
  }
}

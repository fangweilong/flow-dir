import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { MutableRefObject } from 'react'
import type { Settings } from '../persist'
import type { Translate } from '../i18n'
import type { ListData } from '../types'
import { baseName } from '../paths'
import {
  DEFAULT_SORT,
  buildPreset,
  paneIds,
  removePane,
  setRatio,
  splitPane,
  uid,
} from '../workspace'
import type { DropMode, PaneState, TabState, Workspace } from '../workspace'

export type DragState = {
  tabId: string
  fromPane: string
  x: number
  y: number
  started: boolean
  target: { paneId: string; mode: DropMode } | null
}

export type ClosedTabEntry = {
  tab: TabState
  list?: ListData
}

export interface UseWorkspaceLayoutOptions {
  settingsRef: MutableRefObject<Settings>
  listDataRef: MutableRefObject<Record<string, ListData>>
  setListData?: React.Dispatch<React.SetStateAction<Record<string, ListData>>>
  showToast: (msg: string) => void
  t: Translate
  onCancelDir?: (tabId: string) => void
}

export function useWorkspaceLayout({
  settingsRef,
  listDataRef,
  setListData,
  showToast,
  t,
  onCancelDir,
}: UseWorkspaceLayoutOptions) {
  const [ws, setWs] = useState<Workspace | null>(null)
  const [drag, setDrag] = useState<DragState | null>(null)
  const wsRef = useRef<Workspace | null>(null)
  const paneEls = useRef(new Map<string, HTMLDivElement>())
  const homeRef = useRef('C:\\')
  const readyRef = useRef(false)
  const closedTabs = useRef<ClosedTabEntry[]>([])

  useEffect(() => {
    wsRef.current = ws
  }, [ws])

  const registerPane = useCallback((id: string, el: HTMLDivElement | null) => {
    if (el) paneEls.current.set(id, el)
    else paneEls.current.delete(id)
  }, [])

  const activateTab = useCallback((paneId: string, tabId: string) => {
    setWs((prev) =>
      prev
        ? {
            ...prev,
            activePaneId: paneId,
            panes: {
              ...prev.panes,
              [paneId]: { ...prev.panes[paneId], activeTabId: tabId },
            },
          }
        : prev,
    )
  }, [])

  const focusPane = useCallback((paneId: string) => {
    setWs((prev) => (prev && prev.activePaneId !== paneId ? { ...prev, activePaneId: paneId } : prev))
  }, [])

  const applyRatio = useCallback((splitId: string, ratio: number) => {
    setWs((prev) => (prev ? { ...prev, root: setRatio(prev.root, splitId, ratio) } : prev))
  }, [])

  const applyLayout = useCallback((kind: 1 | 2 | 3 | 4, orient: 'h' | 'v') => {
    const current = wsRef.current
    if (!current) return
    const ids = paneIds(current.root)
    if (kind === 1 && ids.length === 1) return
    const kept = ids.slice(0, kind)
    const panes: Record<string, PaneState> = {}
    for (const id of kept) panes[id] = { ...current.panes[id] }
    const tabs: Record<string, TabState> = { ...current.tabs }
    const dropped = ids.slice(kind).flatMap((id) => current.panes[id].tabIds)
    if (dropped.length && kept.length) {
      panes[kept[0]] = { ...panes[kept[0]], tabIds: [...panes[kept[0]].tabIds, ...dropped] }
    }
    const nextIds = [...kept]
    const activePath = tabs[current.panes[current.activePaneId]?.activeTabId]?.path ?? homeRef.current
    while (nextIds.length < kind) {
      const paneId = uid('p')
      const tabId = uid('t')
      tabs[tabId] = { id: tabId, path: activePath, history: [activePath], histIdx: 0 }
      panes[paneId] = {
        id: paneId,
        tabIds: [tabId],
        activeTabId: tabId,
        sort: DEFAULT_SORT,
        hidden: settingsRef.current.showHidden,
      }
      nextIds.push(paneId)
    }
    const root = buildPreset(nextIds, kind, orient)
    const activePaneId = panes[current.activePaneId] ? current.activePaneId : nextIds[0]
    setWs({ root, panes, tabs, activePaneId })
  }, [settingsRef])

  const newTab = useCallback((paneId: string) => {
    if (!wsRef.current) return
    const path = ''
    const tabId = uid('t')
    setWs((prev) =>
      prev
        ? {
            ...prev,
            panes: {
              ...prev.panes,
              [paneId]: {
                ...prev.panes[paneId],
                tabIds: [...prev.panes[paneId].tabIds, tabId],
                activeTabId: tabId,
              },
            },
            tabs: { ...prev.tabs, [tabId]: { id: tabId, path, history: [path], histIdx: 0 } },
            activePaneId: paneId,
          }
        : prev,
    )
  }, [])

  const closeTab = useCallback(
    (paneId: string, tabId: string) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[paneId]
      if (!pane) return
      const closing = current.tabs[tabId]
      if (closing?.path) {
        const limit = Math.max(1, settingsRef.current.closedTabLimit)
        const stack = closedTabs.current
        stack.push({ tab: closing, list: listDataRef.current[tabId] })
        if (stack.length > limit) stack.splice(0, stack.length - limit)
      }
      onCancelDir?.(tabId)
      setListData?.((prev) => {
        if (!(tabId in prev)) return prev
        const next = { ...prev }
        delete next[tabId]
        return next
      })

      const remaining = pane.tabIds.filter((id) => id !== tabId)
      const single = paneIds(current.root).length === 1
      if (!remaining.length && single) {
        const freshId = uid('t')
        setWs((prev) =>
          prev
            ? {
                ...prev,
                tabs: { ...prev.tabs, [freshId]: { id: freshId, path: '', history: [''], histIdx: 0 } },
                panes: {
                  ...prev.panes,
                  [paneId]: { ...prev.panes[paneId], tabIds: [freshId], activeTabId: freshId },
                },
              }
            : prev,
        )
        return
      }

      setWs((prev) => {
        if (!prev) return prev
        const tabs = { ...prev.tabs }
        delete tabs[tabId]
        const panes = { ...prev.panes }
        if (!remaining.length) {
          delete panes[paneId]
          const root = removePane(prev.root, paneId)
          if (!root) return prev
          return {
            ...prev,
            root,
            panes,
            tabs,
            activePaneId: prev.activePaneId === paneId ? paneIds(root)[0] : prev.activePaneId,
          }
        }
        const closedIndex = pane.tabIds.indexOf(tabId)
        const activeTabId =
          pane.activeTabId === tabId ? remaining[Math.min(closedIndex, remaining.length - 1)] : pane.activeTabId
        panes[paneId] = { ...pane, tabIds: remaining, activeTabId }
        return { ...prev, panes, tabs }
      })
    },
    [listDataRef, onCancelDir, setListData, settingsRef],
  )

  const reopenTab = useCallback(() => {
    const current = wsRef.current
    const entry = closedTabs.current.pop()
    if (!current || !entry) {
      showToast(t('toast.nothingToReopen'))
      return
    }
    const tab: TabState = { ...entry.tab, pinned: entry.tab.pinned }
    const paneId = current.panes[current.activePaneId] ? current.activePaneId : Object.keys(current.panes)[0]
    if (!paneId) return
    if (entry.list) {
      setListData?.((prev) => ({ ...prev, [tab.id]: entry.list as ListData }))
    }
    setWs((prev) =>
      prev
        ? {
            ...prev,
            activePaneId: paneId,
            tabs: { ...prev.tabs, [tab.id]: tab },
            panes: {
              ...prev.panes,
              [paneId]: {
                ...prev.panes[paneId],
                tabIds: [...prev.panes[paneId].tabIds, tab.id],
                activeTabId: tab.id,
              },
            },
          }
        : prev,
    )
  }, [setListData, showToast, t])

  const openTabAt = useCallback((paneId: string, path: string) => {
    const tabId = uid('t')
    setWs((prev) =>
      prev
        ? {
            ...prev,
            tabs: { ...prev.tabs, [tabId]: { id: tabId, path, history: [path], histIdx: 0 } },
            panes: {
              ...prev.panes,
              [paneId]: { ...prev.panes[paneId], tabIds: [...prev.panes[paneId].tabIds, tabId] },
            },
          }
        : prev,
    )
  }, [])

  const closeOthers = useCallback((paneId: string, tabId: string) => {
    setWs((prev) => {
      if (!prev) return prev
      const pane = prev.panes[paneId]
      if (!pane) return prev
      const keep = pane.tabIds.filter((id) => id === tabId || prev.tabs[id]?.pinned)
      const tabs: Record<string, TabState> = {}
      for (const id of keep) tabs[id] = prev.tabs[id]
      return { ...prev, tabs, panes: { ...prev.panes, [paneId]: { ...pane, tabIds: keep, activeTabId: tabId } } }
    })
  }, [])

  const closeRight = useCallback((paneId: string, tabId: string) => {
    setWs((prev) => {
      if (!prev) return prev
      const pane = prev.panes[paneId]
      if (!pane) return prev
      const index = pane.tabIds.indexOf(tabId)
      if (index < 0) return prev
      const keep = pane.tabIds.filter((id, at) => at <= index || prev.tabs[id]?.pinned)
      const tabs: Record<string, TabState> = {}
      for (const id of keep) tabs[id] = prev.tabs[id]
      return { ...prev, tabs, panes: { ...prev.panes, [paneId]: { ...pane, tabIds: keep } } }
    })
  }, [])

  const togglePin = useCallback((paneId: string, tabId: string) => {
    setWs((prev) => {
      if (!prev) return prev
      const pane = prev.panes[paneId]
      const tab = prev.tabs[tabId]
      if (!pane || !tab) return prev
      const pinned = !tab.pinned
      const rest = pane.tabIds.filter((id) => id !== tabId)
      const firstUnpinned = rest.findIndex((id) => !prev.tabs[id]?.pinned)
      const at = pinned ? (firstUnpinned < 0 ? rest.length : firstUnpinned) : rest.length
      const tabIds = [...rest.slice(0, at), tabId, ...rest.slice(at)]
      return {
        ...prev,
        tabs: { ...prev.tabs, [tabId]: { ...tab, pinned } },
        panes: { ...prev.panes, [paneId]: { ...pane, tabIds } },
      }
    })
  }, [])

  const duplicateTab = useCallback((paneId: string, tabId: string) => {
    const current = wsRef.current
    const source = current?.tabs[tabId]
    if (!current || !source) return
    const newId = uid('t')
    setWs((prev) =>
      prev
        ? {
            ...prev,
            tabs: { ...prev.tabs, [newId]: { ...source, id: newId, pinned: false } },
            panes: {
              ...prev.panes,
              [paneId]: {
                ...prev.panes[paneId],
                tabIds: [...prev.panes[paneId].tabIds, newId],
                activeTabId: newId,
              },
            },
          }
        : prev,
    )
  }, [])

  const openInNewPane = useCallback((paneId: string, tabId: string) => {
    const current = wsRef.current
    if (!current) return
    const tab = current.tabs[tabId]
    if (!tab) return
    const newPaneId = uid('p')
    const newTabId = uid('t')
    setWs((prev) =>
      prev
        ? {
            ...prev,
            root: splitPane(prev.root, paneId, 'row', 'after', newPaneId),
            panes: {
              ...prev.panes,
              [newPaneId]: {
                id: newPaneId,
                tabIds: [newTabId],
                activeTabId: newTabId,
                sort: DEFAULT_SORT,
                hidden: settingsRef.current.showHidden,
              },
            },
            tabs: { ...prev.tabs, [newTabId]: { id: newTabId, path: tab.path, history: [tab.path], histIdx: 0 } },
            activePaneId: newPaneId,
          }
        : prev,
    )
  }, [settingsRef])

  const hitTest = useCallback((x: number, y: number) => {
    for (const [id, el] of paneEls.current) {
      const r = el.getBoundingClientRect()
      if (x < r.left || x > r.right || y < r.top || y > r.bottom) continue
      const tabBar = el.querySelector('[data-tabbar]')?.getBoundingClientRect()
      if (tabBar && y <= tabBar.bottom) return { paneId: id, mode: 'center' as DropMode }
      const fx = (x - r.left) / r.width
      const fy = (y - r.top) / r.height
      const mode: DropMode =
        fx < 0.26 ? 'left' : fx > 0.74 ? 'right' : fy < 0.26 ? 'top' : fy > 0.74 ? 'bottom' : 'center'
      return { paneId: id, mode }
    }
    return null
  }, [])

  const dropTab = useCallback(
    (tabId: string, fromPaneId: string, target: { paneId: string; mode: DropMode }) => {
      const current = wsRef.current
      const fromPane = current?.panes[fromPaneId]
      if (target.mode === 'center' && target.paneId === fromPaneId) return
      const dest = current?.panes[target.paneId]
      const keepsRows =
        !!fromPane &&
        !!dest &&
        fromPane.sort.key === dest.sort.key &&
        fromPane.sort.dir === dest.sort.dir &&
        fromPane.hidden === dest.hidden
      if (!keepsRows) {
        setListData?.((prev) => {
          if (!(tabId in prev)) return prev
          const next = { ...prev }
          delete next[tabId]
          return next
        })
      }
      setWs((prev) => {
        if (!prev) return prev
        const from = prev.panes[fromPaneId]
        if (!from) return prev
        if (target.mode === 'center') {
          if (target.paneId === fromPaneId) return prev
          const to = prev.panes[target.paneId]
          if (!to) return prev
          const panes = { ...prev.panes }
          const remaining = from.tabIds.filter((id) => id !== tabId)
          if (remaining.length) {
            panes[fromPaneId] = {
              ...from,
              tabIds: remaining,
              activeTabId: from.activeTabId === tabId ? remaining[0] : from.activeTabId,
            }
          } else {
            delete panes[fromPaneId]
          }
          panes[target.paneId] = { ...to, tabIds: [...to.tabIds, tabId], activeTabId: tabId }
          const root = remaining.length ? prev.root : (removePane(prev.root, fromPaneId) ?? prev.root)
          return { ...prev, root, panes, activePaneId: target.paneId }
        }

        const dir = target.mode === 'left' || target.mode === 'right' ? 'row' : 'col'
        const side = target.mode === 'left' || target.mode === 'top' ? 'before' : 'after'
        const base = prev.panes[target.paneId]
        const newPaneId = uid('p')
        const panes: Record<string, PaneState> = {
          ...prev.panes,
          [newPaneId]: {
            id: newPaneId,
            tabIds: [tabId],
            activeTabId: tabId,
            sort: base?.sort ?? DEFAULT_SORT,
            hidden: base?.hidden ?? false,
          },
        }
        let root = splitPane(prev.root, target.paneId, dir, side, newPaneId)
        let tabs = prev.tabs
        const source = panes[fromPaneId]
        if (source) {
          const remaining = source.tabIds.filter((id) => id !== tabId)
          if (remaining.length) {
            panes[fromPaneId] = {
              ...source,
              tabIds: remaining,
              activeTabId: source.activeTabId === tabId ? remaining[0] : source.activeTabId,
            }
          } else if (fromPaneId === target.paneId) {
            const freshId = uid('t')
            const path = prev.tabs[tabId]?.path ?? homeRef.current
            tabs = { ...prev.tabs, [freshId]: { id: freshId, path, history: [path], histIdx: 0 } }
            panes[fromPaneId] = { ...source, tabIds: [freshId], activeTabId: freshId }
          } else {
            delete panes[fromPaneId]
            root = removePane(root, fromPaneId) ?? { t: 'pane', id: newPaneId }
          }
        }
        return { ...prev, root, panes, tabs, activePaneId: newPaneId }
      })
    },
    [setListData],
  )

  const startTabDrag = useCallback(
    (tabId: string, ev: React.PointerEvent) => {
      const current = wsRef.current
      if (!current) return
      const fromPane = Object.values(current.panes).find((p) => p.tabIds.includes(tabId))?.id
      if (!fromPane) return
      const startX = ev.clientX
      const startY = ev.clientY
      let started = false

      const move = (e: PointerEvent) => {
        if (!started && Math.hypot(e.clientX - startX, e.clientY - startY) < 6) return
        started = true
        setDrag({ tabId, fromPane, x: e.clientX, y: e.clientY, started: true, target: hitTest(e.clientX, e.clientY) })
      }
      const up = (e: PointerEvent) => {
        window.removeEventListener('pointermove', move)
        window.removeEventListener('pointerup', up)
        if (started) {
          const target = hitTest(e.clientX, e.clientY)
          if (target) dropTab(tabId, fromPane, target)
        }
        setDrag(null)
      }
      window.addEventListener('pointermove', move)
      window.addEventListener('pointerup', up)
    },
    [hitTest, dropTab],
  )
  const comparePaths = useMemo(() => {
    if (!ws) return []
    const paths: string[] = []
    const seen = new Set<string>()
    for (const pane of Object.values(ws.panes)) {
      const tab = ws.tabs[pane.activeTabId]
      if (tab?.path && !seen.has(tab.path)) {
        seen.add(tab.path)
        paths.push(tab.path)
      }
    }
    return paths
  }, [ws])

  const openFolders = useMemo(() => {
    if (!ws) return []
    const out: string[] = []
    const seen = new Set<string>()
    for (const tab of Object.values(ws.tabs)) {
      if (tab.path && !seen.has(tab.path)) {
        seen.add(tab.path)
        out.push(tab.path)
      }
    }
    return out
  }, [ws])

  const openTabsForSearch = useMemo(() => {
    if (!ws) return []
    const out: { id: string; title: string; path: string }[] = []
    for (const pane of Object.values(ws.panes)) {
      for (const tabId of pane.tabIds) {
        const tab = ws.tabs[tabId]
        if (tab?.path) {
          out.push({
            id: tab.id,
            title: baseName(tab.path) || tab.path,
            path: tab.path,
          })
        }
      }
    }
    return out
  }, [ws])

  const layoutInfo = useMemo(() => {
    const count = ws ? paneIds(ws.root).length : 1
    const dir = ws?.root.t === 'split' ? ws.root.dir : undefined
    return { count, dir }
  }, [ws])

  return {
    ws,
    setWs,
    wsRef,
    drag,
    paneEls,
    homeRef,
    readyRef,
    registerPane,
    activateTab,
    focusPane,
    applyRatio,
    applyLayout,
    newTab,
    closeTab,
    reopenTab,
    openTabAt,
    closeOthers,
    closeRight,
    togglePin,
    duplicateTab,
    openInNewPane,
    startTabDrag,
    dropTab,
    hitTest,
    comparePaths,
    openFolders,
    openTabsForSearch,
    layoutInfo,
  }
}

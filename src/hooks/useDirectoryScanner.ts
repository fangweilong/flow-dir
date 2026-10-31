import { useCallback, useEffect, useRef, useState } from 'react'
import type { MutableRefObject } from 'react'
import { listen } from '@tauri-apps/api/event'
import { errorKey, makeT } from '../i18n'
import {
  cancelDir,
  drives,
  listDir,
  sortOrder,
  watchClear,
  watchSet,
} from '../api'
import { refreshIconCache } from '../icons'
import type { SortItem } from '../api'
import { EMPTY_LIST, emptyList } from '../types'
import type { Drive, Entry, GitStatus, ListData, SortState } from '../types'
import type { Settings } from '../persist'
import { isDir, pathKey } from '../paths'
import { pushHistory, stepHistory } from '../workspace'
import type { Workspace } from '../workspace'

const KEEP_TABS = 8

const sameEntries = (a: Entry[], b: Entry[]): boolean => {
  if (a.length !== b.length) return false
  for (let i = 0; i < a.length; i++) {
    const x = a[i]
    const y = b[i]
    if (x.name !== y.name || x.size !== y.size || x.mtime !== y.mtime || x.attrs !== y.attrs || x.id !== y.id) {
      return false
    }
  }
  return true
}

export interface UseDirectoryScannerOptions {
  ws: Workspace | null
  wsRef: MutableRefObject<Workspace | null>
  setWs: React.Dispatch<React.SetStateAction<Workspace | null>>
  settingsRef: MutableRefObject<Settings>
  gitByDirRef: MutableRefObject<Record<string, GitStatus>>
  invalidateTree: (dir: string) => void
  invalidateGit: (dir: string) => void
  showToast: (text: string) => void
}

export function useDirectoryScanner({
  ws,
  wsRef,
  setWs,
  settingsRef,
  gitByDirRef,
  invalidateTree,
  invalidateGit,
  showToast,
}: UseDirectoryScannerOptions) {
  const [listData, setListData] = useState<Record<string, ListData>>({})
  const [driveList, setDriveList] = useState<Drive[]>([])

  const listDataRef = useRef<Record<string, ListData>>({})
  const gens = useRef<Record<string, number>>({})
  const sortGens = useRef<Record<string, number>>({})
  const watched = useRef(new Map<string, string>())
  const lastScan = useRef(new Map<string, number>())
  const tabUsed = useRef(new Map<string, number>())
  const lastActiveTab = useRef(new Map<string, string>())
  const rescanTimers = useRef(new Map<string, number>())
  const pendingStructural = useRef(new Map<string, boolean>())

  useEffect(() => {
    listDataRef.current = listData
  }, [listData])

  const refreshDrives = useCallback(() => {
    void drives()
      .then(setDriveList)
      .catch(() => undefined)
  }, [])

  const startScan = useCallback(
    (tabId: string, path: string, sort: SortState, hidden: boolean, keep = false) => {
      if (!path) {
        gens.current[tabId] = (gens.current[tabId] ?? 0) + 1
        lastScan.current.set(tabId, Date.now())
        void cancelDir(tabId).catch(() => undefined)
        setListData((prev) => {
          const old = prev[tabId]
          if (old && !old.loading && old.entries.length === 0 && !old.error && !old.truncated) {
            return prev
          }
          return { ...prev, [tabId]: { ...emptyList(), loading: false } }
        })
        return
      }
      const gen = (gens.current[tabId] ?? 0) + 1
      gens.current[tabId] = gen
      lastScan.current.set(tabId, Date.now())
      const acc: Entry[] = []
      if (!keep) {
        setListData((prev) => ({
          ...prev,
          [tabId]: { ...emptyList(), filter: prev[tabId]?.filter ?? '' },
        }))
      }
      void cancelDir(tabId).catch(() => undefined)
      listDir(path, tabId, gen, sort.key, sort.dir, hidden, (batch) => {
        if (gens.current[tabId] !== gen) return
        if (batch.error) {
          setListData((prev) => ({
            ...prev,
            [tabId]: { ...(prev[tabId] ?? emptyList()), loading: false, error: batch.error },
          }))
          return
        }
        for (const [name, size, mtime, attrs, id] of batch.items) {
          acc.push({ name, size, mtime, attrs, id })
        }
        if (keep && !batch.done) return
        setListData((prev) => {
          const old = prev[tabId] ?? emptyList()
          if (!keep) {
            return {
              ...prev,
              [tabId]: {
                ...old,
                entries: batch.items.length ? acc.slice() : old.entries,
                scanned: batch.scanned,
                loading: !batch.done,
                truncated: batch.truncated,
                error: null,
              },
            }
          }
          if (!old.loading && sameEntries(old.entries, acc) && old.truncated === batch.truncated) {
            return prev
          }
          const names = new Set(acc.map((entry) => entry.name))
          const selected = new Set<string>()
          for (const name of old.selected) if (names.has(name)) selected.add(name)
          return {
            ...prev,
            [tabId]: {
              ...old,
              entries: acc,
              scanned: batch.scanned,
              loading: false,
              truncated: batch.truncated,
              error: null,
              selected,
              anchor: names.has(old.anchor) ? old.anchor : '',
            },
          }
        })
      }).catch((e: unknown) => {
        if (gens.current[tabId] !== gen) return
        setListData((prev) => ({
          ...prev,
          [tabId]: {
            ...(prev[tabId] ?? emptyList()),
            loading: false,
            error: { code: 'IPC', detail: String(e) },
          },
        }))
      })
    },
    [],
  )

  const update = useCallback((tabId: string, fn: (d: ListData) => ListData) => {
    setListData((prev) => ({ ...prev, [tabId]: fn(prev[tabId] ?? EMPTY_LIST) }))
  }, [])

  const navigate = useCallback(
    (tabId: string, next: string) => {
      const current = wsRef.current
      if (!current) return
      const pane = Object.values(current.panes).find((p) => p.tabIds.includes(tabId))
      const tab = current.tabs[tabId]
      if (!pane || !tab) return
      if (tab.path === next) return
      setWs((prev) => {
        const live = prev?.tabs[tabId]
        if (!prev || !live) return prev
        const moved = pushHistory(live, next)
        if (moved === live) return prev
        return { ...prev, tabs: { ...prev.tabs, [tabId]: moved } }
      })
      startScan(tabId, next, pane.sort, pane.hidden)
    },
    [setWs, startScan, wsRef],
  )

  const retryTab = useCallback(
    (tabId: string) => {
      const current = wsRef.current
      if (!current) return
      const pane = Object.values(current.panes).find((p) => p.tabIds.includes(tabId))
      const tab = current.tabs[tabId]
      if (!pane || !tab) return
      startScan(tabId, tab.path, pane.sort, pane.hidden)
    },
    [startScan, wsRef],
  )

  const historyMove = useCallback(
    (tabId: string, delta: number) => {
      const current = wsRef.current
      if (!current) return
      const pane = Object.values(current.panes).find((p) => p.tabIds.includes(tabId))
      const tab = current.tabs[tabId]
      if (!pane || !tab) return
      const moved = stepHistory(tab, delta)
      if (!moved) return
      setWs((prev) => {
        const live = prev?.tabs[tabId]
        if (!prev || !live) return prev
        const next = stepHistory(live, delta)
        return next ? { ...prev, tabs: { ...prev.tabs, [tabId]: next } } : prev
      })
      startScan(tabId, moved.path, pane.sort, pane.hidden)
    },
    [setWs, startScan, wsRef],
  )

  const setPaneSort = useCallback(
    (paneId: string, sort: SortState) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[paneId]
      const tab = current.tabs[pane.activeTabId]
      setWs((prev) =>
        prev ? { ...prev, panes: { ...prev.panes, [paneId]: { ...prev.panes[paneId], sort } } } : prev,
      )
      if (!tab) return
      const source = listDataRef.current[tab.id]?.entries ?? []
      if (!source.length) {
        startScan(tab.id, tab.path, sort, pane.hidden, true)
        return
      }
      const gen = (sortGens.current[tab.id] ?? 0) + 1
      sortGens.current[tab.id] = gen
      const items: SortItem[] = source.map((entry) => ({
        name: entry.name,
        size: entry.size,
        mtime: entry.mtime,
        isDir: isDir(entry.attrs),
      }))
      void sortOrder(items, sort.key, sort.dir)
        .then((order) => {
          if (sortGens.current[tab.id] !== gen) return
          setListData((prev) => {
            const live = prev[tab.id]
            if (!live || live.entries !== source || order.length !== source.length) return prev
            return { ...prev, [tab.id]: { ...live, entries: order.map((i) => source[i]) } }
          })
        })
        .catch(() => {
          if (sortGens.current[tab.id] !== gen) return
          startScan(tab.id, tab.path, sort, pane.hidden, true)
        })
    },
    [setWs, startScan, wsRef],
  )

  const toggleHidden = useCallback(
    (paneId: string) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[paneId]
      const tab = current.tabs[pane.activeTabId]
      const hidden = !pane.hidden
      setWs((prev) => (prev ? { ...prev, panes: { ...prev.panes, [paneId]: { ...prev.panes[paneId], hidden } } } : prev))
      if (tab) startScan(tab.id, tab.path, pane.sort, hidden)
    },
    [setWs, startScan, wsRef],
  )

  const refreshPanes = useCallback(() => {
    const current = wsRef.current
    if (!current) return
    let overview = false
    for (const pane of Object.values(current.panes)) {
      const tab = current.tabs[pane.activeTabId]
      if (!tab) continue
      if (!tab.path) overview = true
      else {
        startScan(tab.id, tab.path, pane.sort, pane.hidden)
        invalidateTree(tab.path)
        invalidateGit(tab.path)
      }
    }
    if (overview) refreshDrives()
  }, [invalidateGit, invalidateTree, refreshDrives, startScan, wsRef])

  const manualRefresh = useCallback(() => {
    refreshIconCache()
    refreshPanes()
  }, [refreshPanes])

  // Watch directories of active tabs
  useEffect(() => {
    if (!ws) return
    const current = new Map<string, string>()
    for (const pane of Object.values(ws.panes)) {
      const tab = ws.tabs[pane.activeTabId]
      if (tab?.path) current.set(pane.id, tab.path)
    }
    for (const [paneId, path] of current) {
      const pane = ws.panes[paneId]
      const tab = pane ? ws.tabs[pane.activeTabId] : undefined
      if (!pane || !tab) continue
      const switched = lastActiveTab.current.get(paneId) !== tab.id
      lastActiveTab.current.set(paneId, tab.id)
      const rewatching = watched.current.get(paneId) !== path
      if (rewatching) {
        invalidateGit(path)
        void watchSet(paneId, path).catch(() => undefined)
      }
      if (switched && listDataRef.current[tab.id] && lastScan.current.has(tab.id)) {
        startScan(tab.id, path, pane.sort, pane.hidden, true)
      }
    }
    for (const paneId of watched.current.keys()) {
      if (!current.has(paneId)) void watchClear(paneId).catch(() => undefined)
    }
    watched.current = current
  }, [ws, invalidateGit, startScan])

  // Watcher cleanup on unmount
  useEffect(() => {
    return () => {
      for (const paneId of watched.current.keys()) {
        void watchClear(paneId).catch(() => undefined)
      }
      watched.current.clear()
    }
  }, [])

  // Directory changes event listener
  useEffect(() => {
    let disposed = false
    const unlistens: (() => void)[] = []
    const register = async () => {
      const u1 = await listen<{ paneId: string; structural: boolean }>('flowdir:dir-changed', (event) => {
        const paneId = event.payload.paneId
        pendingStructural.current.set(
          paneId,
          event.payload.structural || pendingStructural.current.get(paneId) === true,
        )
        const existing = rescanTimers.current.get(paneId)
        if (existing) window.clearTimeout(existing)
        rescanTimers.current.set(
          paneId,
          window.setTimeout(() => {
            rescanTimers.current.delete(paneId)
            const structural = pendingStructural.current.get(paneId) === true
            pendingStructural.current.delete(paneId)
            const current = wsRef.current
            if (!current) return
            const pane = current.panes[paneId]
            const tab = pane ? current.tabs[pane.activeTabId] : undefined
            if (!pane || !tab) return
            if (watched.current.get(paneId) !== tab.path) return
            if (!structural) {
              if (gitByDirRef.current[pathKey(tab.path)]?.root) invalidateGit(tab.path)
              return
            }
            startScan(tab.id, tab.path, pane.sort, pane.hidden, true)
            invalidateTree(tab.path)
            invalidateGit(tab.path)
          }, 250),
        )
      })
      if (disposed) u1()
      else unlistens.push(u1)

      const u2 = await listen<{ paneId: string }>('flowdir:git-changed', (event) => {
        const paneId = event.payload.paneId
        const current = wsRef.current
        if (!current) return
        const pane = current.panes[paneId]
        const tab = pane ? current.tabs[pane.activeTabId] : undefined
        if (tab?.path) {
          invalidateGit(tab.path)
        }
      })
      if (disposed) u2()
      else unlistens.push(u2)
    }
    void register()
    return () => {
      disposed = true
      for (const unlisten of unlistens) unlisten()
      for (const timer of rescanTimers.current.values()) window.clearTimeout(timer)
      rescanTimers.current.clear()
      pendingStructural.current.clear()
    }
  }, [gitByDirRef, invalidateGit, invalidateTree, startScan, wsRef])

  // Directory watcher error listener
  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | undefined
    const warned = new Map<string, number>()
    void listen<{ paneId: string; path: string; code: string; detail: string }>(
      'flowdir:dir-error',
      (event) => {
        const current = wsRef.current
        const pane = current?.panes[event.payload.paneId]
        const tab = pane && current ? current.tabs[pane.activeTabId] : undefined
        if (!current || !pane || !tab?.path) return
        if (watched.current.get(pane.id) !== tab.path) return
        startScan(tab.id, tab.path, pane.sort, pane.hidden, true)
        const last = warned.get(pane.id) ?? 0
        if (Date.now() - last < 60_000) return
        warned.set(pane.id, Date.now())
        showToast(
          event.payload.detail || makeT(settingsRef.current.language)(errorKey(event.payload.code)),
        )
      },
    ).then((fn) => {
      if (disposed) fn()
      else unlisten = fn
    })
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [showToast, startScan, wsRef])
  // Window focus rescan
  useEffect(() => {
    const onFocus = () => {
      refreshDrives()
      const current = wsRef.current
      if (!current) return
      for (const pane of Object.values(current.panes)) {
        const tab = current.tabs[pane.activeTabId]
        if (!tab || !tab.path) continue
        const now = Date.now()
        const scannedAt = listDataRef.current[tab.id]?.loading ? now : 0
        if (now - scannedAt > 3_000) {
          startScan(tab.id, tab.path, pane.sort, pane.hidden, true)
        }
      }
    }
    window.addEventListener('focus', onFocus)
    return () => window.removeEventListener('focus', onFocus)
  }, [refreshDrives, startScan, wsRef])


  // Eviction of inactive tabs
  const evictInactiveTabs = useCallback(
    (wsState: Workspace | null) => {
      if (!wsState) return
      const visible = new Set<string>()
      const now = Date.now()
      for (const pane of Object.values(wsState.panes)) {
        const tab = wsState.tabs[pane.activeTabId]
        if (!tab) continue
        visible.add(tab.id)
        tabUsed.current.set(tab.id, now)
        const cached = listDataRef.current[tab.id]
        if (!cached) {
          if (tab.path) {
            startScan(tab.id, tab.path, pane.sort, pane.hidden)
          } else {
            lastScan.current.set(tab.id, now)
            setListData((prev) => {
              if (prev[tab.id] && !prev[tab.id].loading) return prev
              return { ...prev, [tab.id]: { ...emptyList(), loading: false } }
            })
          }
        }
      }
      setListData((prev) => {
        const keys = Object.keys(prev)
        const hidden = keys.filter((k) => !visible.has(k))
        if (!hidden.length) return prev
        const budget = Math.max(0, KEEP_TABS - visible.size)
        hidden.sort((a, b) => (tabUsed.current.get(b) ?? 0) - (tabUsed.current.get(a) ?? 0))
        const keep = new Set([...visible, ...hidden.slice(0, budget)])
        if (keys.every((k) => keep.has(k))) return prev
        const next: Record<string, ListData> = {}
        for (const k of keys) if (keep.has(k)) next[k] = prev[k]
        return next
      })
    },
    [startScan],
  )

  return {
    listData,
    setListData,
    listDataRef,
    driveList,
    setDriveList,
    refreshDrives,
    startScan,
    update,
    navigate,
    retryTab,
    historyMove,
    setPaneSort,
    toggleHidden,
    refreshPanes,
    manualRefresh,
    evictInactiveTabs,
    lastScan,
    gens,
    cancelDir,
  }
}

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { MutableRefObject } from 'react'
import { listen } from '@tauri-apps/api/event'
import { takeLaunchRequest, trayRecent } from '../api'
import type { LaunchRequest, ListData, SortState } from '../types'
import { nextRevealId, pathKey } from '../paths'
import { samePath, uid } from '../workspace'
import type { Workspace } from '../workspace'

/** How long a launch request waits for its row before giving up (slow network shares). */
const PENDING_SELECT_MS = 15_000

export interface UseLaunchAndTrayOptions {
  ws: Workspace | null
  wsRef: MutableRefObject<Workspace | null>
  setWs: React.Dispatch<React.SetStateAction<Workspace | null>>
  listData: Record<string, ListData>
  update: (tabId: string, fn: (d: ListData) => ListData) => void
  newTab: (paneId: string) => void
  navigate: (tabId: string, path: string) => void
  startScan: (tabId: string, path: string, sort: SortState, hidden: boolean) => void
  setSearchOpen?: (open: boolean) => void
}

export function useLaunchAndTray({
  ws,
  wsRef,
  setWs,
  listData,
  update,
  newTab,
  navigate,
  startScan,
  setSearchOpen,
}: UseLaunchAndTrayOptions) {
  const pendingSelect = useRef<{ tabId: string; name: string; path: string } | null>(null)
  const pendingDeadline = useRef(0)
  // A ref alone cannot wake the consumer: opening a file from the folder already on
  // screen changes no list state, so the request needs its own render signal.
  const [pendingSelectTick, setPendingSelectTick] = useState(0)
  const launchApplied = useRef(false)

  const waitForSelect = useCallback((tabId: string, name: string, path: string) => {
    pendingSelect.current = { tabId, name, path }
    pendingDeadline.current = Date.now() + PENDING_SELECT_MS
    setPendingSelectTick((tick) => tick + 1)
  }, [])

  const openRequest = useCallback(
    (req: LaunchRequest) => {
      const current = wsRef.current
      if (!current) return
      const paneId = current.activePaneId
      const pane = current.panes[paneId]
      if (!pane) return
      const path = (req.path ?? '').trim()
      if (!path) {
        if (req.newWindow) newTab(paneId)
        return
      }
      const select = (req.select ?? '').trim()
      if (req.newWindow) {
        const tabId = uid('t')
        if (select) waitForSelect(tabId, select, path)
        setWs((prev) =>
          prev
            ? {
                ...prev,
                tabs: { ...prev.tabs, [tabId]: { id: tabId, path, history: [path], histIdx: 0 } },
                panes: {
                  ...prev.panes,
                  [paneId]: {
                    ...prev.panes[paneId],
                    tabIds: [...prev.panes[paneId].tabIds, tabId],
                    activeTabId: tabId,
                  },
                },
              }
            : prev,
        )
        startScan(tabId, path, pane.sort, pane.hidden)
        return
      }
      const active = current.tabs[pane.activeTabId]
      if (active) {
        if (select) waitForSelect(active.id, select, path)
        navigate(active.id, path)
      }
    },
    [navigate, newTab, setWs, startScan, waitForSelect, wsRef],
  )

  /** Last applied request sequence; the same launch can arrive twice (event + queue). */
  const lastLaunchSeq = useRef(0)
  const applyLaunch = useCallback(
    (req: LaunchRequest | null) => {
      if (!req || !wsRef.current) return
      const seq = req.seq ?? 0
      if (seq > 0 && seq <= lastLaunchSeq.current) return
      if (seq > 0) lastLaunchSeq.current = seq
      openRequest(req)
    },
    [openRequest, wsRef],
  )

  useEffect(() => {
    let disposed = false
    const unlistens: (() => void)[] = []
    const register = async () => {
      const u1 = await listen<LaunchRequest>('flowdir:open-request', (event) => applyLaunch(event.payload))
      if (disposed) u1()
      else unlistens.push(u1)

      if (setSearchOpen) {
        const u3 = await listen('flowdir:search', () => setSearchOpen(true))
        if (disposed) u3()
        else unlistens.push(u3)
      }
    }
    void register()
    return () => {
      disposed = true
      for (const unlisten of unlistens) unlisten()
    }
  }, [applyLaunch, setSearchOpen])

  useEffect(() => {
    if (!ws || launchApplied.current) return
    launchApplied.current = true
    void takeLaunchRequest()
      .then((req) => applyLaunch(req))
      .catch(() => undefined)
  }, [applyLaunch, ws])

  // A forwarded launch can outrun the event listener while the webview is still loading;
  // the backend keeps it queued, so every focus drains whatever is left. The sequence
  // number makes a request that also arrived as an event idempotent.
  useEffect(() => {
    if (!ws) return
    let disposed = false
    let inFlight = false
    const drain = () => {
      if (inFlight || !wsRef.current) return
      inFlight = true
      void takeLaunchRequest()
        .then((req) => {
          if (!disposed) applyLaunch(req)
        })
        .catch(() => undefined)
        .finally(() => {
          inFlight = false
        })
    }
    window.addEventListener('focus', drain)
    return () => {
      disposed = true
      window.removeEventListener('focus', drain)
    }
  }, [applyLaunch, ws, wsRef])

  // "Open with FlowDir" and `flowdir <path>` ask for one row to be selected. The request
  // is only usable once the folder has finished listing, so it waits here.
  useEffect(() => {
    const pending = pendingSelect.current
    if (!pending) return
    // The request names a folder and a row. If the tab was navigated elsewhere (the
    // folder listing can take a while on a network share), the marker must not select a
    // same-named row in the new folder.
    const tab = ws?.tabs[pending.tabId]
    if (!tab || !samePath(tab.path, pending.path)) {
      pendingSelect.current = null
      return
    }
    const data = listData[pending.tabId]
    if (!data || data.loading) return
    // Shell arguments keep the user's spelling; Windows names are case-insensitive.
    const match = data.entries.find(
      (entry) => entry.name.toLowerCase() === pending.name.toLowerCase(),
    )
    if (match) {
      pendingSelect.current = null
      const name = match.name
      update(pending.tabId, (d) => ({
        ...d,
        selected: new Set([name]),
        anchor: name,
        reveal: { name, id: nextRevealId() },
      }))
      return
    }
    // A kept background refresh publishes its old rows with `loading` false until the
    // final batch lands, so a miss here may be stale. Poll until the deadline instead of
    // dropping the request on the first snapshot.
    const remaining = pendingDeadline.current - Date.now()
    if (remaining <= 0) {
      pendingSelect.current = null
      return
    }
    const timer = window.setTimeout(() => setPendingSelectTick((tick) => tick + 1), Math.min(remaining, 1000))
    return () => window.clearTimeout(timer)
  }, [listData, pendingSelectTick, update, ws])

  // Tray recent directories
  const recentDirs = useMemo(() => {
    if (!ws) return []
    const out: string[] = []
    const seen = new Set<string>()
    const add = (path: string | undefined) => {
      if (!path) return
      const key = pathKey(path)
      if (seen.has(key)) return
      seen.add(key)
      out.push(path)
    }
    for (const pane of Object.values(ws.panes)) {
      for (const id of pane.tabIds) add(ws.tabs[id]?.path)
    }
    for (const tab of Object.values(ws.tabs)) {
      for (let i = tab.history.length - 1; i >= 0; i--) add(tab.history[i])
    }
    return out.slice(0, 8)
  }, [ws])

  const recentKey = recentDirs.join('\n')
  // The list is rebuilt on every workspace change, but the tray only cares when the
  // contents change: read it from a ref so focusing panes or switching tabs does not
  // rebuild the native menu.
  const recentDirsRef = useRef(recentDirs)
  recentDirsRef.current = recentDirs
  useEffect(() => {
    void trayRecent(recentDirsRef.current).catch(() => undefined)
  }, [recentKey])

  return {
    pendingSelect,
    pendingSelectTick,
    openRequest,
    recentDirs,
  }
}

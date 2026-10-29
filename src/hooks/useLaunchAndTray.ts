import { useCallback, useEffect, useMemo, useRef } from 'react'
import type { MutableRefObject } from 'react'
import { listen } from '@tauri-apps/api/event'
import { takeLaunchRequest, trayRecent } from '../api'
import type { LaunchRequest, SortState } from '../types'
import { pathKey } from '../paths'
import { uid } from '../workspace'
import type { Workspace } from '../workspace'

export interface UseLaunchAndTrayOptions {
  ws: Workspace | null
  wsRef: MutableRefObject<Workspace | null>
  setWs: React.Dispatch<React.SetStateAction<Workspace | null>>
  newTab: (paneId: string) => void
  navigate: (tabId: string, path: string) => void
  startScan: (tabId: string, path: string, sort: SortState, hidden: boolean) => void
}

export function useLaunchAndTray({
  ws,
  wsRef,
  setWs,
  newTab,
  navigate,
  startScan,
}: UseLaunchAndTrayOptions) {
  const pendingSelect = useRef<{ tabId: string; name: string } | null>(null)
  const launchApplied = useRef(false)

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
        if (select) pendingSelect.current = { tabId, name: select }
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
        if (select) pendingSelect.current = { tabId: active.id, name: select }
        navigate(active.id, path)
      }
    },
    [navigate, newTab, setWs, startScan, wsRef],
  )

  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | undefined
    void listen<LaunchRequest>('flowdir:launch-request', (event) => openRequest(event.payload)).then((fn) => {
      if (disposed) fn()
      else unlisten = fn
    })
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [openRequest])

  useEffect(() => {
    if (!ws || launchApplied.current) return
    launchApplied.current = true
    void takeLaunchRequest()
      .then((req) => {
        if (req) openRequest(req)
      })
      .catch(() => undefined)
  }, [openRequest, ws])

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
  useEffect(() => {
    void trayRecent(recentDirs).catch(() => undefined)
  }, [recentKey, recentDirs])

  return {
    pendingSelect,
    openRequest,
    recentDirs,
  }
}

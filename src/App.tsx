import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { PointerEvent as ReactPointerEvent } from 'react'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import iconUrl from './assets/icon.png'
import { PaneView } from './PaneView'
import type { MenuPoint } from './PaneView'
import { ContextMenu } from './ContextMenu'
import type { MenuItem } from './ContextMenu'
import { OpsPanel } from './OpsPanel'
import { PromptDialog } from './PromptDialog'
import { PreviewPanel } from './PreviewPanel'
import { SearchOverlay } from './SearchOverlay'
import { SettingsModal } from './SettingsModal'
import { SidebarTree } from './SidebarTree'
import {
  cancelDir,
  clipboardCopy,
  clipboardPaste,
  drives,
  explorerOpen,
  layoutLoad,
  layoutPreserve,
  layoutSave,
  listDir,
  openPath,
  opCancel,
  opStart,
  previewExternal,
  previewInfo,
  quickAccess,
  setAppTheme,
  settingsLoad,
  settingsSave,
  shellMenuShow,
  shellProperties,
  titlebarMenu,
  treeDirs,
  updateCheck,
  gitStatus,
  watchClear,
  watchSet,
} from './api'
import { EMPTY_LIST, emptyList } from './types'
import type {
  Drive,
  Entry,
  GitStatus,
  Lang,
  ListData,
  OpKind,
  OpProgress,
  Place,
  PreviewInfo,
  SortState,
  UpdateStatus,
} from './types'
import { driveKindLabel, makeT } from './i18n'
import { DEFAULT_SETTINGS, fromSnapshot, toSnapshot } from './persist'
import type { Settings } from './persist'
import { isDir, isFsPath, joinPath, pathKey, treeChain } from './paths'
import { centerOffset } from './scroll'
import { formatBinding, isTextInput, matchBinding, resolveShortcuts } from './shortcuts'
import {
  DEFAULT_SORT,
  adoptIds,
  buildPreset,
  createWorkspace,
  paneIds,
  removePane,
  setRatio,
  splitPane,
  uid,
} from './workspace'
import type { DropMode, Node, PaneState, TabState, Workspace } from './workspace'

/** Folders whose full change list stays cached; older ones refetch on demand. */
const GIT_CACHE_LIMIT = 64

type DragState = {
  tabId: string
  fromPane: string
  x: number
  y: number
  started: boolean
  target: { paneId: string; mode: DropMode } | null
}

type SplitNode = Extract<Node, { t: 'split' }>

/** Below these widths a release collapses the panel instead of leaving it unusable. */
const SIDEBAR_MIN = 140
const PREVIEW_MIN = 220
/** Splitter width; the centre-pane half-width cap must account for it. */
const GUTTER = 3

export default function App() {
  const [ws, setWs] = useState<Workspace | null>(null)
  const [listData, setListData] = useState<Record<string, ListData>>({})
  const [places, setPlaces] = useState<Place[]>([])
  const [driveList, setDriveList] = useState<Drive[]>([])
  const [drag, setDrag] = useState<DragState | null>(null)
  const [settings, setSettings] = useState<Settings>(DEFAULT_SETTINGS)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [updateAvailable, setUpdateAvailable] = useState<string | null>(null)
  const [searchOpen, setSearchOpen] = useState(false)
  const [previewOpen, setPreviewOpen] = useState(true)
  const [sidebarWidth, setSidebarWidth] = useState(DEFAULT_SETTINGS.sidebarWidth)
  const [previewWidth, setPreviewWidth] = useState(DEFAULT_SETTINGS.previewWidth)
  const [maximized, setMaximized] = useState(false)
  useEffect(() => {
    // Alt+Space normally opens the system menu from the caption we no longer have.
    const onKey = (ev: KeyboardEvent) => {
      if (!ev.altKey || ev.code !== 'Space') return
      ev.preventDefault()
      const dpr = window.devicePixelRatio || 1
      void titlebarMenu(Math.round((window.screenX + 16) * dpr), Math.round((window.screenY + 44) * dpr))
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  useEffect(() => {
    const win = getCurrentWindow()
    void win.isMaximized().then(setMaximized)
    const unlisten = win.onResized(() => {
      void win.isMaximized().then(setMaximized)
    })
    return () => {
      void unlisten.then((off) => off())
    }
  }, [])
  const [preview, setPreview] = useState<{ info: PreviewInfo | null; loading: boolean; error: { code: string; detail?: string } | null }>({
    info: null,
    loading: false,
    error: null,
  })
  const [toast, setToast] = useState<string | null>(null)
  const [menu, setMenu] = useState<{ x: number; y: number; items: MenuItem[] } | null>(null)
  const [prompt, setPrompt] = useState<{ title: string; initial: string; submit: (value: string) => void } | null>(null)
  const [ops, setOps] = useState<Record<string, OpProgress>>({})
  /** Sidebar tree: expanded folders (key → display path) and their cached children. */
  const [treeOpen, setTreeOpen] = useState<Record<string, string>>({})
  const [treeNodes, setTreeNodes] = useState<Record<string, string[]>>({})
  /** `git status` per folder, keyed by {@link pathKey}; empty repo = no badges. */
  const [gitByDir, setGitByDir] = useState<Record<string, GitStatus>>({})
  /** Bumped by invalidation so the fetch effect re-arms even if nothing was cached yet. */
  const [gitEpoch, setGitEpoch] = useState(0)

  const wsRef = useRef<Workspace | null>(null)
  const layoutRef = useRef<HTMLDivElement>(null)
  const settingsRef = useRef<Settings>(DEFAULT_SETTINGS)
  const gens = useRef<Record<string, number>>({})
  const paneEls = useRef(new Map<string, HTMLDivElement>())
  const homeRef = useRef('C:\\')
  const readyRef = useRef(false)
  const watched = useRef(new Map<string, string>())
  const rescanTimers = useRef(new Map<string, number>())
  const toastTimer = useRef<number | null>(null)
  const notifiedUpdate = useRef<string | null>(null)
  const previewRequest = useRef(0)
  const treePending = useRef(new Set<string>())
  /** Per-folder request generation; a reply from an older mode is dropped. */
  const treeGeneration = useRef(new Map<string, number>())
  const treeCounter = useRef(0)
  const treeOpenRef = useRef<Record<string, string>>({})
  const gitPending = useRef(new Set<string>())
  /** Bumped on invalidation so a reply that started earlier is discarded. */
  const gitSeq = useRef(new Map<string, number>())
  const sidebarRef = useRef<HTMLElement | null>(null)
  const treeNodesRef = useRef<Record<string, string[]>>({})

  const lang: Lang = settings.language
  const t = useMemo(() => makeT(lang), [lang])
  const keys = useMemo(() => resolveShortcuts(settings.shortcuts), [settings.shortcuts])

  useEffect(() => {
    wsRef.current = ws
  }, [ws])

  useEffect(() => {
    settingsRef.current = settings
  }, [settings])

  /**
   * Fetches one tree node. Callers only ask for folders that are open, so the cache
   * never grows with folders the user never looked at; a repeated request while one
   * is in flight is dropped instead of queueing.
   */
  const loadTree = useCallback((path: string, force = false) => {
    const key = pathKey(path)
    if (!isFsPath(path) || treePending.current.has(key)) return
    if (!force && treeNodesRef.current[key]) return
    treePending.current.add(key)
    const generation = ++treeCounter.current
    treeGeneration.current.set(key, generation)
    const reply = (names: string[]) => {
      if (treeGeneration.current.get(key) !== generation) return
      treeNodesRef.current = { ...treeNodesRef.current, [key]: names }
      setTreeNodes(treeNodesRef.current)
    }
    treeDirs(path, settingsRef.current.showHidden)
      .then(reply)
      .catch(() => reply([]))
      .finally(() => {
        if (treeGeneration.current.get(key) === generation) treePending.current.delete(key)
      })
  }, [])

  /** Drops one folder's children; open nodes refetch on the next pass. */
  const invalidateTree = useCallback((path: string) => {
    const key = pathKey(path)
    if (!(key in treeNodesRef.current)) return
    const next = { ...treeNodesRef.current }
    delete next[key]
    treeNodesRef.current = next
    setTreeNodes(next)
  }, [])

  /**
   * Expanding paints the cache first and refreshes behind it, so a long-unseen folder
   * never flashes empty and never shows a folder that was deleted meanwhile.
   */
  const toggleTree = useCallback(
    (path: string) => {
      const key = pathKey(path)
      const opening = !(key in treeOpenRef.current)
      if (opening) treeOpenRef.current = { ...treeOpenRef.current, [key]: path }
      setTreeOpen((prev) => {
        if (!(key in prev)) return { ...prev, [key]: path }
        const next = { ...prev }
        delete next[key]
        return next
      })
      if (opening) loadTree(path, true)
    },
    [loadTree],
  )

  // Single funnel for fetches: expanding, an invalidation and the hidden-files toggle
  // all just leave open nodes without children, and this pass fills the gaps.
  useEffect(() => {
    treeOpenRef.current = treeOpen
    if (!settings.sidebarOpen) return
    for (const [key, path] of Object.entries(treeOpen)) {
      if (!treeNodes[key]) loadTree(path)
    }
  }, [settings.sidebarOpen, treeOpen, treeNodes, loadTree])

  useEffect(() => {
    // Replies requested under the previous hidden mode must not repopulate the cleared
    // cache, so bump the generation of every folder that is cached or in flight.
    for (const key of new Set([...Object.keys(treeNodesRef.current), ...treePending.current])) {
      treeGeneration.current.set(key, ++treeCounter.current)
    }
    treeNodesRef.current = {}
    treePending.current.clear()
    setTreeNodes({})
  }, [settings.showHidden])

  /** One `git status` per visited folder; failures cache as "no repository". */
  const loadGit = useCallback((dir: string) => {
    const key = pathKey(dir)
    if (!dir || gitPending.current.has(key)) return
    const seq = (gitSeq.current.get(key) ?? 0) + 1
    gitSeq.current.set(key, seq)
    gitPending.current.add(key)
    const store = (status: GitStatus) =>
      setGitByDir((prev) => {
        if (gitSeq.current.get(key) !== seq) return prev
        const next = { ...prev, [key]: status }
        // Bound the cache; entries are pruned to the folder, but folders are unbounded.
        const keys = Object.keys(next)
        for (const stale of keys.slice(0, Math.max(0, keys.length - GIT_CACHE_LIMIT))) delete next[stale]
        return next
      })
    gitStatus(dir)
      .then(store)
      .catch(() => store({ root: '', files: [], dirs: [] }))
      .finally(() => {
        if (gitSeq.current.get(key) === seq) gitPending.current.delete(key)
      })
  }, [])

  /** Drops cached status (and any reply still in flight) so the next pass re-runs git. */
  const invalidateGit = useCallback((dir: string) => {
    const key = pathKey(dir)
    gitSeq.current.set(key, (gitSeq.current.get(key) ?? 0) + 1)
    gitPending.current.delete(key)
    setGitEpoch((epoch) => epoch + 1)
    setGitByDir((prev) => {
      if (!(key in prev)) return prev
      const next = { ...prev }
      delete next[key]
      return next
    })
  }, [])

  // Badges are cosmetic: wait out fast arrow-key scrolling so holding a key does not
  // spawn one `git status` per folder passed through.
  useEffect(() => {
    if (!ws) return
    const missing: string[] = []
    for (const pane of Object.values(ws.panes)) {
      const tab = ws.tabs[pane.activeTabId]
      if (tab?.path && !(pathKey(tab.path) in gitByDir)) missing.push(tab.path)
    }
    if (!missing.length) return
    const timer = window.setTimeout(() => {
      for (const dir of missing) loadGit(dir)
    }, 150)
    return () => window.clearTimeout(timer)
  }, [ws, gitByDir, gitEpoch, loadGit])

  const showToast = useCallback((text: string) => {
    setToast(text)
    if (toastTimer.current) window.clearTimeout(toastTimer.current)
    toastTimer.current = window.setTimeout(() => setToast(null), 2200)
  }, [])

  /** Shared by F3 and the preview panel button; the backend owns the per-extension rules. */
  const openExternalPreview = useCallback(
    (target: string) => {
      void previewExternal(target).catch((err: unknown) => {
        const code = err && typeof err === 'object' && 'code' in err ? String(err.code) : ''
        showToast(t(code === 'NO_RULE' ? 'preview.externalMissing' : 'preview.externalFailed', { code }))
      })
    },
    [showToast, t],
  )

  const startScan = useCallback((tabId: string, path: string, sort: SortState, hidden: boolean) => {
    // The drive overview is rendered from the drive list, not from a directory scan:
    // retire the previous folder's scan so it stops enumerating and cannot write into
    // this tab any more.
    if (!path) {
      gens.current[tabId] = (gens.current[tabId] ?? 0) + 1
      void cancelDir(tabId).catch(() => undefined)
      setListData((prev) => ({ ...prev, [tabId]: { ...emptyList(), loading: false } }))
      return
    }
    const gen = (gens.current[tabId] ?? 0) + 1
    gens.current[tabId] = gen
    const acc: Entry[] = []
    setListData((prev) => ({
      ...prev,
      [tabId]: { ...emptyList(), filter: prev[tabId]?.filter ?? '' },
    }))
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
      setListData((prev) => ({
        ...prev,
        [tabId]: {
          ...(prev[tabId] ?? emptyList()),
          entries: batch.items.length ? acc.slice() : (prev[tabId]?.entries ?? []),
          scanned: batch.scanned,
          loading: !batch.done,
          error: null,
        },
      }))
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
  }, [])

  useEffect(() => {
    let cancelled = false
    void (async () => {
      const [savedSettings, placeList, driveListResult, saved] = await Promise.all([
        settingsLoad().catch(() => DEFAULT_SETTINGS),
        quickAccess().catch(() => [] as Place[]),
        drives().catch(() => [] as Drive[]),
        layoutLoad().catch(() => null),
      ])
      if (cancelled) return
      const merged = { ...DEFAULT_SETTINGS, ...savedSettings }
      setSettings(merged)
      setPreviewOpen(merged.previewOpen)
      setSidebarWidth(merged.sidebarWidth >= SIDEBAR_MIN ? merged.sidebarWidth : DEFAULT_SETTINGS.sidebarWidth)
      setPreviewWidth(merged.previewWidth >= PREVIEW_MIN ? merged.previewWidth : DEFAULT_SETTINGS.previewWidth)
      setPlaces(placeList)
      setDriveList(driveListResult)
      const home = placeList[0]?.path ?? 'C:\\'
      homeRef.current = home
      const restored = fromSnapshot(saved)
      if (saved && !restored) void layoutPreserve().catch(() => undefined)
      const initial = restored ?? createWorkspace(home, savedSettings.showHidden)
      adoptIds(initial)
      setWs(initial)
      readyRef.current = true
    })()
    return () => {
      cancelled = true
    }
  }, [])

  // WebView2 draws its own page menu (reload / save as / print / inspect) wherever the app
  // has no menu of its own. Swallow it outside text fields, which keep the native edit menu.
  useEffect(() => {
    const onContextMenu = (ev: MouseEvent) => {
      const target = ev.target instanceof Element ? ev.target : null
      if (target?.closest('input, textarea, [contenteditable="true"]')) return
      ev.preventDefault()
    }
    window.addEventListener('contextmenu', onContextMenu)
    return () => window.removeEventListener('contextmenu', onContextMenu)
  }, [])

  useEffect(() => {
    if (!ws) return
    const visible = new Set<string>()
    for (const pane of Object.values(ws.panes)) {
      const tab = ws.tabs[pane.activeTabId]
      if (!tab) continue
      visible.add(tab.id)
      if (!listData[tab.id]) startScan(tab.id, tab.path, pane.sort, pane.hidden)
    }
    setListData((prev) => {
      const keys = Object.keys(prev)
      if (!keys.some((k) => !visible.has(k))) return prev
      const next: Record<string, ListData> = {}
      for (const k of keys) if (visible.has(k)) next[k] = prev[k]
      return next
    })
  }, [ws, listData, startScan])

  useEffect(() => {
    if (!ws || !readyRef.current) return
    const timer = window.setTimeout(() => {
      void layoutSave(toSnapshot(ws)).catch(() => undefined)
    }, 500)
    return () => window.clearTimeout(timer)
  }, [ws])

  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | undefined
    void listen('flowdir:flush-layout', () => {
      const current = wsRef.current
      if (current) void layoutSave(toSnapshot(current)).catch(() => undefined)
    }).then((fn) => {
      if (disposed) fn()
      else unlisten = fn
    })
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])

  useEffect(() => {
    const announce = (status: UpdateStatus) => {
      if (!status.available || !status.latest) return
      setUpdateAvailable(status.latest)
      if (notifiedUpdate.current === status.latest) return
      notifiedUpdate.current = status.latest
      showToast(makeT(settingsRef.current.language)('toast.updateAvailable', { version: status.latest }))
    }
    let disposed = false
    let unlisten: (() => void) | undefined
    void listen<UpdateStatus>('flowdir:update', (event) => announce(event.payload)).then((fn) => {
      if (disposed) fn()
      else unlisten = fn
    })
    // The scheduler wakes 20s after launch; ask once for the cached release so the badge
    // appears immediately without waiting for it.
    const timer = window.setTimeout(() => {
      void updateCheck(false)
        .then((status) => {
          if (!disposed) announce(status)
        })
        .catch(() => undefined)
    }, 3000)
    return () => {
      disposed = true
      unlisten?.()
      window.clearTimeout(timer)
    }
  }, [showToast])

  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    const apply = () => {
      const dark = settings.theme === 'dark' || (settings.theme === 'system' && media.matches)
      document.documentElement.classList.toggle('dark', dark)
    }
    apply()
    void setAppTheme(settings.theme).catch(() => undefined)
    media.addEventListener('change', apply)
    return () => media.removeEventListener('change', apply)
  }, [settings.theme])

  useEffect(() => {
    if (!ws) return
    const current = new Map<string, string>()
    for (const pane of Object.values(ws.panes)) {
      const tab = ws.tabs[pane.activeTabId]
      if (tab?.path) current.set(pane.id, tab.path)
    }
    for (const [paneId, path] of current) {
      if (watched.current.get(paneId) !== path) {
        void watchSet(paneId, path).catch(() => undefined)
      }
    }
    for (const paneId of watched.current.keys()) {
      if (!current.has(paneId)) void watchClear(paneId).catch(() => undefined)
    }
    watched.current = current
  }, [ws])

  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | undefined
    void listen<{ paneId: string }>('flowdir:dir-changed', (event) => {
      const paneId = event.payload.paneId
      const existing = rescanTimers.current.get(paneId)
      if (existing) window.clearTimeout(existing)
      rescanTimers.current.set(
        paneId,
        window.setTimeout(() => {
          rescanTimers.current.delete(paneId)
          const current = wsRef.current
          if (!current) return
          const pane = current.panes[paneId]
          const tab = pane ? current.tabs[pane.activeTabId] : undefined
          if (!pane || !tab) return
          if (watched.current.get(paneId) !== tab.path) return
          startScan(tab.id, tab.path, pane.sort, pane.hidden)
          invalidateTree(tab.path)
          invalidateGit(tab.path)
        }, 250),
      )
    }).then((fn) => {
      if (disposed) fn()
      else unlisten = fn
    })
    return () => {
      disposed = true
      unlisten?.()
      for (const timer of rescanTimers.current.values()) window.clearTimeout(timer)
      rescanTimers.current.clear()
    }
  }, [invalidateGit, invalidateTree, startScan])

  const updateSettings = useCallback((next: Settings) => {
    setSettings(next)
    void settingsSave(next).catch(() => undefined)
  }, [])

  /** Ctrl+B and the caption button share this so a collapsed drag reopens at a usable width. */
  const toggleSidebar = useCallback(() => {
    const current = settingsRef.current
    if (current.sidebarOpen) {
      updateSettings({ ...current, sidebarOpen: false })
      return
    }
    const width = Math.max(SIDEBAR_MIN, current.sidebarWidth)
    setSidebarWidth(width)
    updateSettings({ ...current, sidebarOpen: true, sidebarWidth: width })
  }, [updateSettings])

  // The centre panes keep at least half of the layout: each side panel is capped at
  // `50% - the other panel`, so opening both still leaves the panes the majority.
  const startSidebarResize = useCallback(
    (ev: ReactPointerEvent<HTMLDivElement>) => {
      ev.preventDefault()
      const rect = layoutRef.current?.getBoundingClientRect()
      if (!rect) return
      const startX = ev.clientX
      const startWidth = sidebarWidth
      const max = Math.max(
        0,
        rect.width / 2 - GUTTER * (previewOpen ? 2 : 1) - (previewOpen ? previewWidth : 0),
      )
      let next = startWidth
      let moved = false
      const move = (e: PointerEvent) => {
        moved = true
        next = Math.min(max, Math.max(0, startWidth + e.clientX - startX))
        setSidebarWidth(next)
      }
      const end = () => {
        window.removeEventListener('pointermove', move)
        window.removeEventListener('pointerup', end)
        window.removeEventListener('pointercancel', end)
        if (!moved) return
        if (next < Math.min(SIDEBAR_MIN, max)) {
          setSidebarWidth(settingsRef.current.sidebarWidth)
          updateSettings({ ...settingsRef.current, sidebarOpen: false })
        } else {
          updateSettings({ ...settingsRef.current, sidebarWidth: Math.round(next) })
        }
      }
      window.addEventListener('pointermove', move)
      window.addEventListener('pointerup', end)
      window.addEventListener('pointercancel', end)
    },
    [sidebarWidth, previewOpen, previewWidth, updateSettings],
  )

  const startPreviewResize = useCallback(
    (ev: ReactPointerEvent<HTMLDivElement>) => {
      ev.preventDefault()
      const rect = layoutRef.current?.getBoundingClientRect()
      if (!rect) return
      const startX = ev.clientX
      const startWidth = previewWidth
      const sidebarOpen = settingsRef.current.sidebarOpen
      const max = Math.max(
        0,
        rect.width / 2 - GUTTER * (sidebarOpen ? 2 : 1) - (sidebarOpen ? sidebarWidth : 0),
      )
      let next = startWidth
      let moved = false
      const move = (e: PointerEvent) => {
        moved = true
        next = Math.min(max, Math.max(0, startWidth + startX - e.clientX))
        setPreviewWidth(next)
      }
      const end = () => {
        window.removeEventListener('pointermove', move)
        window.removeEventListener('pointerup', end)
        window.removeEventListener('pointercancel', end)
        if (!moved) return
        if (next < Math.min(PREVIEW_MIN, max)) {
          setPreviewWidth(settingsRef.current.previewWidth)
          setPreviewOpen(false)
        } else {
          updateSettings({ ...settingsRef.current, previewWidth: Math.round(next) })
        }
      }
      window.addEventListener('pointermove', move)
      window.addEventListener('pointerup', end)
      window.addEventListener('pointercancel', end)
    },
    [previewWidth, sidebarWidth, updateSettings],
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
      const stack = tab.history.slice(0, tab.histIdx + 1)
      if (stack[stack.length - 1] !== next) stack.push(next)
      setWs((prev) =>
        prev
          ? {
              ...prev,
              tabs: {
                ...prev.tabs,
                [tabId]: { ...tab, path: next, history: stack, histIdx: stack.length - 1 },
              },
            }
          : prev,
      )
      startScan(tabId, next, pane.sort, pane.hidden)
    },
    [startScan],
  )

  /** Tree rows: folders navigate the active tab, non-path locations go to the shell. */
  const openTreePath = useCallback(
    (path: string) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[current.activePaneId]
      const tab = pane ? current.tabs[pane.activeTabId] : undefined
      if (!tab) return
      if (!isFsPath(path)) {
        void openPath(path).catch(() => undefined)
        return
      }
      navigate(tab.id, path)
    },
    [navigate],
  )

  const setPaneSort = useCallback(
    (paneId: string, sort: SortState) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[paneId]
      const tab = current.tabs[pane.activeTabId]
      setWs((prev) => (prev ? { ...prev, panes: { ...prev.panes, [paneId]: { ...prev.panes[paneId], sort } } } : prev))
      if (tab) startScan(tab.id, tab.path, sort, pane.hidden)
    },
    [startScan],
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
    [startScan],
  )

  const newTab = useCallback((paneId: string) => {
    if (!wsRef.current) return
    // New tabs open on the drive overview (C:, D:, …), never inherit the current folder.
    const path = ''
    const tabId = uid('t')
    setWs((prev) =>
      prev
        ? {
            ...prev,
            panes: {
              ...prev.panes,
              [paneId]: { ...prev.panes[paneId], tabIds: [...prev.panes[paneId].tabIds, tabId], activeTabId: tabId },
            },
            tabs: { ...prev.tabs, [tabId]: { id: tabId, path, history: [path], histIdx: 0 } },
            activePaneId: paneId,
          }
        : prev,
    )
  }, [])

  const closeTab = useCallback((paneId: string, tabId: string) => {
    const current = wsRef.current
    if (!current) return
    const pane = current.panes[paneId]
    if (!pane) return
    void cancelDir(tabId).catch(() => undefined)
    delete gens.current[tabId]
    setListData((prev) => {
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
              panes: { ...prev.panes, [paneId]: { ...prev.panes[paneId], tabIds: [freshId], activeTabId: freshId } },
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
  }, [])

  const activateTab = useCallback((paneId: string, tabId: string) => {
    setWs((prev) =>
      prev ? { ...prev, activePaneId: paneId, panes: { ...prev.panes, [paneId]: { ...prev.panes[paneId], activeTabId: tabId } } } : prev,
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
  }, [])

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
      setListData((prev) => {
        if (!(tabId in prev)) return prev
        const next = { ...prev }
        delete next[tabId]
        return next
      })
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
            // Splitting the pane the only tab came from: keep the origin pane
            // alive with a fresh tab, otherwise the new split would collapse.
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
    [],
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
        if (!started && Math.hypot(e.clientX - startX, e.clientY - startY) < 4) return
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

  const registerPane = useCallback((id: string, el: HTMLDivElement | null) => {
    if (el) paneEls.current.set(id, el)
    else paneEls.current.delete(id)
  }, [])

  const copySelection = useCallback(
    (paneId: string, cut: boolean) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[paneId]
      const tab = pane ? current.tabs[pane.activeTabId] : undefined
      const data = tab ? listData[tab.id] : undefined
      if (!pane || !tab || !data || data.selected.size === 0) return
      const paths = Array.from(data.selected).map((name) => joinPath(tab.path, name))
      clipboardCopy(paths, cut)
        .then(() => showToast(makeT(lang)(cut ? 'toast.cut' : 'toast.copied', { count: paths.length })))
        .catch(() => showToast(makeT(lang)('toast.copyFailed')))
    },
    [listData, lang, showToast],
  )

  const refreshPanes = useCallback(() => {
    const current = wsRef.current
    if (!current) return
    let overview = false
    for (const pane of Object.values(current.panes)) {
      const tab = current.tabs[pane.activeTabId]
      if (!tab) continue
      // The overview has no scan to restart; re-enumerate the drives instead.
      if (!tab.path) overview = true
      else {
        startScan(tab.id, tab.path, pane.sort, pane.hidden)
        invalidateTree(tab.path)
        invalidateGit(tab.path)
      }
    }
    if (overview) void drives().then(setDriveList).catch(() => undefined)
  }, [invalidateGit, invalidateTree, startScan])

  const runOp = useCallback(
    (kind: OpKind, sources: string[], options?: { targetDir?: string; newName?: string; permanent?: boolean }) => {
      if (!sources.length) return
      const opId = uid('op')
      setOps((prev) => ({
        ...prev,
        [opId]: {
          opId,
          phase: kind,
          current: 0,
          total: sources.length,
          name: null,
          work: 0,
          workTotal: 0,
          done: false,
          aborted: false,
          error: null,
        },
      }))
      const drop = (delay: number) =>
        window.setTimeout(() => {
          setOps((prev) => {
            const next = { ...prev }
            delete next[opId]
            return next
          })
        }, delay)
      opStart({ opId, kind, sources, ...options }, (progress) => {
        setOps((prev) => (prev[opId] ? { ...prev, [opId]: progress } : prev))
        if (!progress.done) return
        if (progress.error) {
          showToast(t('op.failed', { code: progress.error.code }))
          drop(5000)
        } else {
          if (progress.aborted) showToast(t('op.canceled'))
          else refreshPanes()
          drop(progress.aborted ? 2500 : 800)
        }
      }).catch((e: unknown) => {
        showToast(t('op.failed', { code: String(e) }))
        drop(4000)
      })
    },
    [refreshPanes, showToast, t],
  )

  const pathsFor = useCallback((paneId: string, names: string[]): string[] => {
    const current = wsRef.current
    if (!current) return []
    const pane = current.panes[paneId]
    const tab = pane ? current.tabs[pane.activeTabId] : undefined
    if (!tab?.path) return []
    return names.map((name) => joinPath(tab.path, name))
  }, [])

  const deleteEntries = useCallback(
    (paneId: string, names: string[], permanent: boolean) => {
      runOp('delete', pathsFor(paneId, names), { permanent })
    },
    [pathsFor, runOp],
  )

  const openProperties = useCallback(
    (paneId: string, names: string[]) => {
      const paths = pathsFor(paneId, names)
      for (const path of paths)
        void shellProperties(path).catch((e: unknown) => {
          const code = e && typeof e === 'object' && 'code' in e ? String((e as { code: unknown }).code) : String(e)
          showToast(t('op.failed', { code }))
        })
    },
    [pathsFor, showToast, t],
  )

  const startRename = useCallback(
    (paneId: string, name: string) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[paneId]
      const tab = pane ? current.tabs[pane.activeTabId] : undefined
      if (!tab?.path) return
      const target = joinPath(tab.path, name)
      setPrompt({
        title: t('dialog.renameTitle'),
        initial: name,
        submit: (value) => runOp('rename', [target], { newName: value }),
      })
    },
    [runOp, t],
  )

  const createFolder = useCallback(
    (paneId: string) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[paneId]
      const tab = pane ? current.tabs[pane.activeTabId] : undefined
      if (!tab?.path) return
      setPrompt({
        title: t('dialog.newFolderTitle'),
        initial: '',
        submit: (value) => runOp('newdir', [tab.path], { targetDir: tab.path, newName: value }),
      })
    },
    [runOp, t],
  )

  const pasteInto = useCallback(
    (paneId: string) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[paneId]
      const tab = pane ? current.tabs[pane.activeTabId] : undefined
      if (!tab?.path) return
      void clipboardPaste()
        .then((clip) => {
          if (!clip.paths.length) {
            showToast(t('toast.noClipboard'))
            return
          }
          runOp(clip.cut ? 'move' : 'copy', clip.paths, { targetDir: tab.path })
        })
        .catch(() => showToast(t('toast.noClipboard')))
    },
    [runOp, showToast, t],
  )

  const copyPaths = useCallback(
    (paths: string[]) => {
      const text = paths.join('\r\n')
      void navigator.clipboard
        .writeText(text)
        .then(() => showToast(t('toast.copied', { count: paths.length })))
        .catch(() => undefined)
    },
    [showToast, t],
  )

  const closeOthers = useCallback((paneId: string, tabId: string) => {
    setWs((prev) => {
      if (!prev) return prev
      const pane = prev.panes[paneId]
      if (!pane) return prev
      const tabs = { ...prev.tabs }
      for (const id of pane.tabIds) if (id !== tabId) delete tabs[id]
      return { ...prev, tabs, panes: { ...prev.panes, [paneId]: { ...pane, tabIds: [tabId], activeTabId: tabId } } }
    })
  }, [])

  const closeRight = useCallback((paneId: string, tabId: string) => {
    setWs((prev) => {
      if (!prev) return prev
      const pane = prev.panes[paneId]
      if (!pane) return prev
      const index = pane.tabIds.indexOf(tabId)
      if (index < 0) return prev
      const keep = pane.tabIds.slice(0, index + 1)
      const tabs = { ...prev.tabs }
      for (const id of pane.tabIds.slice(index + 1)) delete tabs[id]
      return { ...prev, tabs, panes: { ...prev.panes, [paneId]: { ...pane, tabIds: keep } } }
    })
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
  }, [])

  const rowFallback = useCallback(
    (paneId: string, entry: Entry, paths: string[]): MenuItem[] => {
      const dir = isDir(entry.attrs)
      return [
        {
          kind: 'item',
          label: t('menu.open'),
          onSelect: () => {
            const current = wsRef.current
            const pane = current?.panes[paneId]
            const tab = pane ? current?.tabs[pane.activeTabId] : undefined
            if (!tab) return
            const target = joinPath(tab.path, entry.name)
            if (dir) navigate(tab.id, target)
            else void openPath(target).catch(() => undefined)
          },
        },
        {
          kind: 'item',
          label: t('menu.copy'),
          onSelect: () => {
            clipboardCopy(paths, false)
              .then(() => showToast(t('toast.copied', { count: paths.length })))
              .catch(() => undefined)
          },
        },
        {
          kind: 'item',
          label: t('menu.cut'),
          onSelect: () => {
            clipboardCopy(paths, true)
              .then(() => showToast(t('toast.cut', { count: paths.length })))
              .catch(() => undefined)
          },
        },
        { kind: 'item', label: t('menu.paste'), onSelect: () => pasteInto(paneId) },
        { kind: 'sep' },
        { kind: 'item', label: t('menu.rename'), hint: 'F2', onSelect: () => startRename(paneId, entry.name) },
        {
          kind: 'item',
          label: t('menu.delete'),
          hint: 'Del',
          danger: true,
          onSelect: () => deleteEntries(paneId, [entry.name], false),
        },
        {
          kind: 'item',
          label: t('menu.deletePermanent'),
          hint: 'Shift+Del',
          danger: true,
          onSelect: () => deleteEntries(paneId, [entry.name], true),
        },
        { kind: 'sep' },
        { kind: 'item', label: t('menu.copyPath'), onSelect: () => copyPaths(paths) },
      ]
    },
    [copyPaths, deleteEntries, openInNewPane, pasteInto, showToast, startRename, t],
  )

  const openRowMenu = useCallback(
    (paneId: string, entry: Entry, point: MenuPoint) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[paneId]
      const tab = pane ? current.tabs[pane.activeTabId] : undefined
      if (!tab) return
      const selected = listData[tab.id]?.selected
      const names = selected && selected.has(entry.name) ? Array.from(selected) : [entry.name]
      const paths = names.map((name) => joinPath(tab.path, name))
      const dpr = window.devicePixelRatio || 1
      void shellMenuShow(
        paths,
        Math.round(point.screenX * dpr),
        Math.round(point.screenY * dpr),
        point.shiftKey,
      )
        .then((outcome) => {
          if (outcome.invoked) refreshPanes()
        })
        .catch(() => {
          setMenu({ x: point.clientX, y: point.clientY, items: rowFallback(paneId, entry, paths) })
        })
    },
    [listData, refreshPanes, rowFallback],
  )

  const openBlankMenu = useCallback(
    (paneId: string, point: MenuPoint) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[paneId]
      const tab = pane ? current.tabs[pane.activeTabId] : undefined
      if (!tab) return
      const data = listData[tab.id]
      // The drive overview has no folder to create, paste, or reveal in.
      const items: MenuItem[] = tab.path
        ? [
            { kind: 'item', label: t('menu.newFolder'), onSelect: () => createFolder(paneId) },
            { kind: 'item', label: t('menu.paste'), hint: 'Ctrl+V', onSelect: () => pasteInto(paneId) },
            { kind: 'sep' },
            { kind: 'item', label: t('menu.refresh'), hint: 'F5', onSelect: () => refreshPanes() },
            {
              kind: 'item',
              label: t('menu.selectAll'),
              hint: 'Ctrl+A',
              onSelect: () => {
                const all = (data?.entries ?? []).map((entry) => entry.name)
                update(tab.id, (d) => ({ ...d, selected: new Set(all), anchor: 0 }))
              },
            },
            { kind: 'sep' },
            {
              kind: 'item',
              label: t('menu.openInExplorer'),
              onSelect: () => void explorerOpen(tab.path).catch(() => undefined),
            },
            {
              kind: 'item',
              label: pane.hidden ? t('list.hideHidden') : t('list.showHidden'),
              onSelect: () => toggleHidden(paneId),
            },
          ]
        : [{ kind: 'item', label: t('menu.refresh'), hint: 'F5', onSelect: () => refreshPanes() }]
      setMenu({ x: point.clientX, y: point.clientY, items })
    },
    [createFolder, listData, pasteInto, refreshPanes, t, toggleHidden, update],
  )

  const openTabMenu = useCallback(
    (paneId: string, tabId: string, point: MenuPoint) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[paneId]
      const tab = current.tabs[tabId]
      if (!pane || !tab) return
      const index = pane.tabIds.indexOf(tabId)
      const items: MenuItem[] = [
        { kind: 'item', label: t('menu.closeTab'), onSelect: () => closeTab(paneId, tabId) },
        {
          kind: 'item',
          label: t('menu.closeOthers'),
          disabled: pane.tabIds.length < 2,
          onSelect: () => closeOthers(paneId, tabId),
        },
        {
          kind: 'item',
          label: t('menu.closeRight'),
          disabled: index >= pane.tabIds.length - 1,
          onSelect: () => closeRight(paneId, tabId),
        },
        { kind: 'sep' },
        { kind: 'item', label: t('menu.copyPath'), disabled: !tab.path, onSelect: () => copyPaths([tab.path]) },
        { kind: 'item', label: t('menu.openInNewPane'), onSelect: () => openInNewPane(paneId, tabId) },
      ]
      setMenu({ x: point.clientX, y: point.clientY, items })
    },
    [closeOthers, closeRight, closeTab, copyPaths, openInNewPane, t],
  )

  const activePane = ws?.panes[ws.activePaneId]
  const activeTab = activePane ? ws?.tabs[activePane.activeTabId] : undefined

  /**
   * Keeps the active folder centred in the sidebar instead of parking it on the bottom
   * edge the way `scrollIntoView({block:'nearest'})` does. Rows that are already fully
   * visible are left alone, so a background refresh never yanks the panel around.
   */
  const revealTreeRow = useCallback((path: string, force: boolean) => {
    const scroller = sidebarRef.current
    if (!scroller) return
    const row = scroller.querySelector<HTMLElement>(`[data-tree-key="${CSS.escape(pathKey(path))}"]`)
    if (!row) return
    const box = scroller.getBoundingClientRect()
    const rect = row.getBoundingClientRect()
    const margin = 16
    const visible = rect.top >= box.top + margin && rect.bottom <= box.bottom - margin
    if (!force && visible) return
    scroller.scrollTo({
      top: centerOffset(rect.top, rect.height, box.top, box.height, scroller.scrollTop),
      behavior: 'smooth',
    })
  }, [])

  const layoutInfo = useMemo(() => {
    if (!ws) return { count: 1, dir: 'row' as const }
    return { count: paneIds(ws.root).length, dir: ws.root.t === 'split' ? ws.root.dir : ('row' as const) }
  }, [ws])

  const activeList = activeTab ? listData[activeTab.id] : undefined
  const selectedNames = activeList ? Array.from(activeList.selected) : []
  const selectionKey = selectedNames.join('\u0001')

  /** The single selected folder, if the list selection points at one. */
  const selectedFolderPath = useMemo(() => {
    const path = activeTab?.path
    if (!path || !activeList || activeList.selected.size !== 1) return ''
    const name = activeList.selected.values().next().value
    if (typeof name !== 'string') return ''
    const entry = activeList.entries.find((item) => item.name === name)
    return entry && isDir(entry.attrs) ? joinPath(path, name) : ''
  }, [activeList, activeTab?.path])

  /**
   * What the tree reveals: a selected folder, else the current folder. The folder itself
   * is expanded too, so its subfolders stay visible while a file inside it is selected.
   */
  const treeFocus = selectedFolderPath || activeTab?.path || ''

  // Highlight follows the selection: a selected folder wins; selecting a file (or several
  // items) clears it instead of pretending the parent folder is what the user picked.
  const treeSelected =
    selectedFolderPath || (activeList && activeList.selected.size ? '' : (activeTab?.path ?? ''))

  useEffect(() => {
    if (!treeFocus) return
    setTreeOpen((prev) => {
      let next: Record<string, string> | null = null
      for (const dir of treeChain(treeFocus)) {
        const key = pathKey(dir)
        if (!(key in prev)) (next ??= { ...prev })[key] = dir
      }
      return next ?? prev
    })
  }, [treeFocus])

  // Focusing a folder always re-centres; loading children only recentres if needed.
  useEffect(() => {
    if (!settings.sidebarOpen || !treeFocus) return
    const frame = window.requestAnimationFrame(() => revealTreeRow(treeFocus, true))
    return () => window.cancelAnimationFrame(frame)
  }, [treeFocus, settings.sidebarOpen, revealTreeRow])

  useEffect(() => {
    if (!settings.sidebarOpen || !treeFocus) return
    const frame = window.requestAnimationFrame(() => revealTreeRow(treeFocus, false))
    return () => window.cancelAnimationFrame(frame)
  }, [treeNodes, treeFocus, settings.sidebarOpen, revealTreeRow])
  const previewName = selectedNames.length === 1 ? selectedNames[0] : ''

  useEffect(() => {
    if (!previewOpen || !activeTab || !previewName) {
      setPreview({ info: null, loading: false, error: null })
      return
    }
    const target = joinPath(activeTab.path, previewName)
    const request = ++previewRequest.current
    setPreview({ info: null, loading: true, error: null })
    const timer = window.setTimeout(() => {
      const dark = document.documentElement.classList.contains('dark')
      previewInfo(target, 320, dark ? '0f172a' : 'ffffff')
        .then((info) => {
          if (previewRequest.current === request) setPreview({ info, loading: false, error: null })
        })
        .catch((e: unknown) => {
          if (previewRequest.current === request) {
            const error =
              e && typeof e === 'object' && 'code' in e
                ? (e as { code: string; detail?: string })
                : { code: 'PREVIEW', detail: typeof e === 'string' ? e : '' }
            setPreview({ info: null, loading: false, error })
          }
        })
    }, 120)
    return () => window.clearTimeout(timer)
  }, [previewOpen, previewName, activeTab, settings.theme, settings.externalPreview, selectionKey])

  // Window-level shortcuts all go through the registry, so a rebind in Settings applies to
  // the global commands and the panes alike. Overlays return early to keep the background
  // list from reacting to keys typed into them.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (settingsOpen || searchOpen) return
      const bare = !event.ctrlKey && !event.altKey && !event.shiftKey && !event.metaKey
      if (bare && isTextInput(event.target)) return

      if (matchBinding(event, keys.search)) {
        event.preventDefault()
        setSearchOpen(true)
        return
      }
      if (matchBinding(event, keys.togglePreview)) {
        event.preventDefault()
        setPreviewOpen((open) => !open)
        return
      }
      if (matchBinding(event, keys.toggleSidebar)) {
        event.preventDefault()
        toggleSidebar()
        return
      }
      if (matchBinding(event, keys.openSettings)) {
        event.preventDefault()
        setSettingsOpen(true)
        return
      }
      if (activePane && matchBinding(event, keys.newTab)) {
        event.preventDefault()
        newTab(activePane.id)
        return
      }
      if (activePane && activeTab && matchBinding(event, keys.closeTab)) {
        event.preventDefault()
        closeTab(activePane.id, activeTab.id)
        return
      }
      if (matchBinding(event, keys.externalPreview)) {
        if (!activeTab || !previewName) return
        const entry = activeList?.entries.find((item) => item.name === previewName)
        if (!entry || isDir(entry.attrs)) return
        event.preventDefault()
        openExternalPreview(joinPath(activeTab.path, previewName))
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [
    keys,
    toggleSidebar,
    settingsOpen,
    searchOpen,
    activePane,
    activeTab,
    activeList,
    previewName,
    newTab,
    closeTab,
    openExternalPreview,
  ])

  const placeLabel = (key: string) => {
    switch (key) {
      case 'home':
        return t('place.home')
      case 'desktop':
        return t('place.desktop')
      case 'downloads':
        return t('place.downloads')
      case 'documents':
        return t('place.documents')
      default:
        return key
    }
  }

  type QuickPlaceEntry = { key: string; label: string; path: string; custom: boolean }

  const quickPlaces = useMemo<QuickPlaceEntry[]>(() => {
    const builtin = places
      .filter((place) => !settings.hiddenPlaces.includes(place.key))
      .map((place) => ({ key: place.key, label: placeLabel(place.key), path: place.path, custom: false }))
    const custom = settings.customPlaces.map((place) => ({
      key: `custom:${place.path}`,
      label: place.name || place.path,
      path: place.path,
      custom: true,
    }))
    return [...builtin, ...custom]
    // placeLabel is derived from t, which is derived from lang.
  }, [places, settings.hiddenPlaces, settings.customPlaces, lang])

  const removePlace = useCallback(
    (place: QuickPlaceEntry) => {
      const next = place.custom
        ? { ...settings, customPlaces: settings.customPlaces.filter((entry) => entry.path !== place.path) }
        : { ...settings, hiddenPlaces: Array.from(new Set([...settings.hiddenPlaces, place.key])) }
      updateSettings(next)
    },
    [settings, updateSettings],
  )

  const openPlaceMenu = useCallback(
    (place: QuickPlaceEntry, point: { clientX: number; clientY: number }) => {
      const items: MenuItem[] = [
        { kind: 'item', label: t('menu.open'), onSelect: () => activeTab && navigate(activeTab.id, place.path) },
        { kind: 'sep' },
        { kind: 'item', label: t('sidebar.removePlace'), danger: true, onSelect: () => removePlace(place) },
      ]
      setMenu({ x: point.clientX, y: point.clientY, items })
    },
    [activeTab, navigate, removePlace, t],
  )

  const openDriveMenu = useCallback(
    (paneId: string, point: MenuPoint) => {
      const current = wsRef.current
      const pane = current ? current.panes[paneId] : undefined
      const tab = pane && current ? current.tabs[pane.activeTabId] : undefined
      if (!tab) return
      const items: MenuItem[] = driveList.map((drive) => ({
        kind: 'item' as const,
        label: drive.name,
        hint: driveKindLabel(t, drive.kind),
        onSelect: () =>
          isFsPath(drive.root)
            ? navigate(tab.id, drive.root)
            : void openPath(drive.root).catch(() => undefined),
      }))
      setMenu({ x: point.clientX, y: point.clientY, items })
    },
    [driveList, navigate, lang],
  )

  const renderNode = useCallback(
    (node: Node): React.ReactNode => {
      if (!ws) return null
      if (node.t === 'pane') {
        const pane = ws.panes[node.id]
        if (!pane) return null
        return (
          <div
            key={node.id}
            ref={(el) => registerPane(node.id, el)}
            className="flex min-h-0 min-w-0 flex-1 flex-col"
          >
            <PaneView
              pane={pane}
              tabs={ws.tabs}
              data={listData[pane.activeTabId]}
              git={gitByDir[pathKey(ws.tabs[pane.activeTabId]?.path ?? '')] ?? null}
              active={ws.activePaneId === pane.id}
              lang={lang}
              dropHint={drag?.target?.paneId === pane.id ? drag.target.mode : null}
              draggingTabId={drag?.tabId ?? null}
              onFocusPane={() => focusPane(pane.id)}
              onNewTab={() => newTab(pane.id)}
              onCloseTab={(tabId) => closeTab(pane.id, tabId)}
              onActivateTab={(tabId) => activateTab(pane.id, tabId)}
              onNavigate={(tabId, path) => navigate(tabId, path)}
              onTabDragStart={startTabDrag}
              onSortChange={(sort) => setPaneSort(pane.id, sort)}
              onToggleHidden={() => toggleHidden(pane.id)}
              onClipboard={(cut) => copySelection(pane.id, cut)}
              onRowContextMenu={(entry, point) => openRowMenu(pane.id, entry, point)}
              onBlankContextMenu={(point) => openBlankMenu(pane.id, point)}
              onTabContextMenu={(tabId, point) => openTabMenu(pane.id, tabId, point)}
              drives={driveList}
              onDriveMenu={(point) => openDriveMenu(pane.id, point)}
              onProperties={(names) => openProperties(pane.id, names)}
              onRename={(name) => startRename(pane.id, name)}
              onDelete={(names, permanent) => deleteEntries(pane.id, names, permanent)}
              onPaste={() => pasteInto(pane.id)}
              onRefresh={refreshPanes}
              keys={keys}
              update={update}
            />
          </div>
        )
      }
      return <SplitView key={node.id} node={node} render={renderNode} onRatio={applyRatio} />
    },
    [
      ws,
      listData,
      drag,
      lang,
      focusPane,
      newTab,
      closeTab,
      activateTab,
      navigate,
      startTabDrag,
      setPaneSort,
      toggleHidden,
      copySelection,
      update,
      registerPane,
      applyRatio,
      openRowMenu,
      openBlankMenu,
      openTabMenu,
      openProperties,
      startRename,
      deleteEntries,
      pasteInto,
      refreshPanes,
      keys,
      gitByDir,
      driveList,
      openDriveMenu,
    ],
  )

  return (
    <div className="flex h-full flex-col bg-surface-2 text-fg">
      <header
        data-tauri-drag-region
        onContextMenu={(ev) => {
          ev.preventDefault()
          const dpr = window.devicePixelRatio || 1
          void titlebarMenu(Math.round(ev.screenX * dpr), Math.round(ev.screenY * dpr))
        }}
        className="flex h-10 shrink-0 items-center gap-2 border-b border-line bg-surface/90 pl-2.5"
      >
        <div className="flex items-center gap-2" data-tauri-drag-region>
          <img src={iconUrl} alt="" className="h-6 w-6 select-none" draggable={false} />
          <span data-tauri-drag-region className="text-[13px] font-semibold tracking-tight">
            {t('app.name')}
          </span>
        </div>

        <div className="mx-2 h-5 w-px bg-line" />

        <div className="flex items-center gap-0.5 rounded-md bg-surface-3 p-0.5">
          <LayoutButton title={t('layout.single')} active={layoutInfo.count === 1} onClick={() => applyLayout(1, 'h')} kind={1} />
          <LayoutButton
            title={t('layout.twoH')}
            active={layoutInfo.count === 2 && layoutInfo.dir === 'row'}
            onClick={() => applyLayout(2, 'h')}
            kind={2}
            dir="row"
          />
          <LayoutButton
            title={t('layout.twoV')}
            active={layoutInfo.count === 2 && layoutInfo.dir === 'col'}
            onClick={() => applyLayout(2, 'v')}
            kind={2}
            dir="col"
          />
          <LayoutButton title={t('layout.three')} active={layoutInfo.count === 3} onClick={() => applyLayout(3, 'h')} kind={3} />
          <LayoutButton title={t('layout.four')} active={layoutInfo.count === 4} onClick={() => applyLayout(4, 'h')} kind={4} />
        </div>

        <div data-tauri-drag-region className="ml-2 min-w-0 flex-1 truncate text-[11px] text-muted-2">
          {activeTab ? activeTab.path : ''}
        </div>
        <div data-tauri-drag-region className="hidden shrink-0 text-[11px] text-muted-2 lg:block">
          {t('layout.hint', { count: layoutInfo.count })}
        </div>
        <button
          type="button"
          onClick={toggleSidebar}
          title={t('sidebar.toggle')}
          className={`grid h-7 w-7 shrink-0 place-items-center rounded-md hover:bg-surface-3 ${
            settings.sidebarOpen ? 'text-muted' : 'text-muted-2'
          }`}
        >
          <svg viewBox="0 0 16 16" className="h-4 w-4" fill="none" stroke="currentColor" strokeWidth="1.4">
            <rect x="2" y="3" width="12" height="10" rx="1.5" />
            <path d="M6 3v10" />
          </svg>
        </button>
        <button
          type="button"
          onClick={() => setSearchOpen(true)}
          title="Ctrl+F"
          className="grid h-7 w-7 shrink-0 place-items-center rounded-md text-muted hover:bg-surface-3 hover:text-fg"
        >
          <svg viewBox="0 0 16 16" className="h-4 w-4" fill="none" stroke="currentColor" strokeWidth="1.5">
            <circle cx="7" cy="7" r="4.2" />
            <path d="m10.2 10.2 3 3" strokeLinecap="round" />
          </svg>
        </button>
        <button
          type="button"
          onClick={() => setPreviewOpen((open) => !open)}
          title="Ctrl+P"
          className={`grid h-7 w-7 shrink-0 place-items-center rounded-md hover:bg-surface-3 ${
            previewOpen ? 'text-blue-500' : 'text-muted'
          }`}
        >
          <svg viewBox="0 0 16 16" className="h-4 w-4" fill="none" stroke="currentColor" strokeWidth="1.4">
            <rect x="2" y="2.5" width="12" height="11" rx="1.5" />
            <path d="M10 2.5v11" />
          </svg>
        </button>
        <button
          type="button"
          onClick={() => setSettingsOpen(true)}
          title={t('app.settings')}
          className="relative grid h-7 w-7 shrink-0 place-items-center rounded-md text-muted hover:bg-surface-3 hover:text-fg"
        >
          <svg viewBox="0 0 16 16" className="h-4 w-4" fill="none" stroke="currentColor" strokeWidth="1.5">
            <path d="M2 4h7M12.2 4H14M2 8h3M7.8 8H14M2 12h7M12.2 12H14" strokeLinecap="round" />
            <circle cx="10.6" cy="4" r="1.5" />
            <circle cx="6.2" cy="8" r="1.5" />
            <circle cx="10.6" cy="12" r="1.5" />
          </svg>
          {updateAvailable && (
            <span
              data-update-badge
              title={t('settings.updateAvailable', { version: updateAvailable })}
              className="absolute right-1 top-1 h-1.5 w-1.5 rounded-full bg-blue-500"
            />
          )}
        </button>

        <div className="ml-1 flex h-full shrink-0 items-stretch">
          <CaptionButton label="最小化" onClick={() => void getCurrentWindow().minimize()}>
            <path d="M3 8h10" strokeLinecap="round" />
          </CaptionButton>
          <CaptionButton
            label={maximized ? '还原' : '最大化'}
            onClick={() => {
              const win = getCurrentWindow();
              void win.toggleMaximize().then(() => win.isMaximized().then(setMaximized))
            }}
          >
            {maximized ? (
              <>
                <rect x="3" y="5" width="8" height="8" rx="1.2" />
                <path d="M5.5 5V4.2A1.2 1.2 0 0 1 6.7 3h4.1A1.2 1.2 0 0 1 12 4.2v4.1" />
              </>
            ) : (
              <rect x="3.5" y="3.5" width="9" height="9" rx="1.2" />
            )}
          </CaptionButton>
          <CaptionButton label="关闭" danger onClick={() => void getCurrentWindow().close()}>
            <path d="M4 4l8 8M12 4l-8 8" strokeLinecap="round" />
          </CaptionButton>
        </div>
      </header>

      <div ref={layoutRef} className="flex min-h-0 flex-1">
        {settings.sidebarOpen ? (
          <>
            <aside
              ref={sidebarRef}
              style={{
                width: sidebarWidth,
                maxWidth: previewOpen
                  ? `calc(50% - ${GUTTER * 2}px - ${previewWidth}px)`
                  : `calc(50% - ${GUTTER}px)`,
              }}
              className="flex shrink-0 flex-col gap-4 overflow-y-auto overflow-x-hidden border-r border-line bg-surface-2 px-2 py-3"
            >
              <section>
                <h2 className="px-2 pb-1 text-[10px] font-semibold tracking-wider text-muted-2">
                  {t('sidebar.quickAccess')}
                </h2>
                {quickPlaces.map((place) => (
                  <SidebarItem
                    key={place.key}
                    active={activeTab?.path === place.path}
                    label={place.label}
                    hint={place.custom ? '' : undefined}
                    onClick={() => activeTab && navigate(activeTab.id, place.path)}
                    onContextMenu={(point) => openPlaceMenu(place, point)}
                    icon={
                      <svg viewBox="0 0 16 16" className="h-4 w-4 text-blue-500" fill="currentColor">
                        <path d="M1.5 3.6c0-.6.5-1.1 1.1-1.1h3.2c.4 0 .7.15.95.42l.85.93h6.3c.6 0 1.1.5 1.1 1.1v7.4c0 .6-.5 1.1-1.1 1.1H2.6c-.6 0-1.1-.5-1.1-1.1V3.6Z" />
                      </svg>
                    }
                  />
                ))}
              </section>
              <section>
                <h2 className="px-2 pb-1 text-[10px] font-semibold tracking-wider text-muted-2">
                  {t('sidebar.thisPc')}
                </h2>
                <SidebarTree
                  drives={driveList}
                  open={treeOpen}
                  nodes={treeNodes}
                  selectedPath={treeSelected}
                  currentPath={activeTab?.path ?? ''}
                  onToggle={toggleTree}
                  onOpen={openTreePath}
                />
              </section>
            </aside>
            <ResizeHandle onPointerDown={startSidebarResize} />
          </>
        ) : null}

        <main className="flex min-h-0 min-w-0 flex-1 gap-1 p-1">{ws && renderNode(ws.root)}</main>
        {previewOpen && (
          <>
            <ResizeHandle onPointerDown={startPreviewResize} />
            <PreviewPanel
              width={previewWidth}
              maxWidth={
                settings.sidebarOpen
                  ? `calc(50% - ${GUTTER * 2}px - ${sidebarWidth}px)`
                  : `calc(50% - ${GUTTER}px)`
              }
              info={preview.info}
              loading={preview.loading}
              error={preview.error}
              name={previewName}
              path={activeTab ? joinPath(activeTab.path, previewName) : ''}
              lang={lang}
              externalKey={formatBinding(keys.externalPreview)}
              onClose={() => setPreviewOpen(false)}
              onOpenExternal={openExternalPreview}
            />
          </>
        )}
      </div>

      {drag?.started && ws && (
        <div
          className="pointer-events-none fixed z-50 max-w-[220px] truncate rounded-md border border-blue-300 bg-surface/95 px-2 py-1 text-[11px] text-fg shadow-lg"
          style={{ left: drag.x + 12, top: drag.y + 10 }}
        >
          {ws.tabs[drag.tabId]?.path
            ? (ws.tabs[drag.tabId].path.split('\\').filter(Boolean).pop() ?? '—')
            : t('sidebar.thisPc')}
        </div>
      )}

      {toast && (
        <div className="pointer-events-none fixed bottom-4 left-1/2 z-50 -translate-x-1/2 rounded-md bg-slate-900/90 px-3 py-1.5 text-[11.5px] text-white shadow-lg dark:bg-slate-100/95 dark:text-slate-900">
          {toast}
        </div>
      )}

      {searchOpen && activeTab && (
        <SearchOverlay
          root={activeTab.path || homeRef.current}
          lang={lang}
          preferEverything={settings.useEverything}
          onClose={() => setSearchOpen(false)}
          onOpen={(folder) => navigate(activeTab.id, folder)}
        />
      )}

      <OpsPanel ops={Object.values(ops)} lang={lang} onCancel={(opId) => void opCancel(opId)} />

      {menu && <ContextMenu x={menu.x} y={menu.y} items={menu.items} onClose={() => setMenu(null)} />}

      {prompt && (
        <PromptDialog
          title={prompt.title}
          initial={prompt.initial}
          lang={lang}
          onConfirm={(value) => {
            prompt.submit(value)
            setPrompt(null)
          }}
          onClose={() => setPrompt(null)}
        />
      )}

      {settingsOpen && (
        <SettingsModal
          settings={settings}
          lang={lang}
          onChange={updateSettings}
          onClose={() => setSettingsOpen(false)}
        />
      )}
    </div>
  )
}

function ResizeHandle({ onPointerDown }: { onPointerDown: (ev: ReactPointerEvent<HTMLDivElement>) => void }) {
  return (
    <div
      role="separator"
      aria-orientation="vertical"
      onPointerDown={onPointerDown}
      style={{ width: GUTTER }}
      className="shrink-0 cursor-col-resize bg-line-strong hover:bg-blue-400"
    />
  )
}

function SplitView({
  node,
  render,
  onRatio,
}: {
  node: SplitNode
  render: (n: Node) => React.ReactNode
  onRatio: (splitId: string, ratio: number) => void
}) {
  const ref = useRef<HTMLDivElement>(null)
  return (
    <div ref={ref} className={`flex min-h-0 min-w-0 flex-1 ${node.dir === 'row' ? 'flex-row' : 'flex-col'}`}>
      <div className="flex min-h-0 min-w-0" style={{ flexGrow: node.ratio, flexBasis: 0 }}>
        {render(node.a)}
      </div>
      <div
        onPointerDown={(ev) => {
          ev.preventDefault()
          const container = ref.current
          if (!container) return
          const move = (e: PointerEvent) => {
            const rect = container.getBoundingClientRect()
            const raw =
              node.dir === 'row' ? (e.clientX - rect.left) / rect.width : (e.clientY - rect.top) / rect.height
            onRatio(node.id, Math.min(0.85, Math.max(0.15, raw)))
          }
          const up = () => {
            window.removeEventListener('pointermove', move)
            window.removeEventListener('pointerup', up)
          }
          window.addEventListener('pointermove', move)
          window.addEventListener('pointerup', up)
        }}
        className={`shrink-0 bg-line-strong hover:bg-blue-400 ${
          node.dir === 'row' ? 'w-[3px] cursor-col-resize' : 'h-[3px] cursor-row-resize'
        }`}
      />
      <div className="flex min-h-0 min-w-0" style={{ flexGrow: 1 - node.ratio, flexBasis: 0 }}>
        {render(node.b)}
      </div>
    </div>
  )
}

function LayoutButton({
  title,
  active,
  onClick,
  kind,
  dir,
}: {
  title: string
  active: boolean
  onClick: () => void
  kind: 1 | 2 | 3 | 4
  dir?: 'row' | 'col'
}) {
  const rects: React.ReactNode[] = []
  if (kind === 1) {
    rects.push(<rect key="a" x="2" y="2" width="12" height="12" rx="1.5" />)
  } else if (kind === 2 && dir === 'row') {
    rects.push(<rect key="a" x="2" y="2" width="5.4" height="12" rx="1.2" />)
    rects.push(<rect key="b" x="8.6" y="2" width="5.4" height="12" rx="1.2" />)
  } else if (kind === 2) {
    rects.push(<rect key="a" x="2" y="2" width="12" height="5.4" rx="1.2" />)
    rects.push(<rect key="b" x="2" y="8.6" width="12" height="5.4" rx="1.2" />)
  } else if (kind === 3) {
    rects.push(<rect key="a" x="2" y="2" width="5.4" height="12" rx="1.2" />)
    rects.push(<rect key="b" x="8.6" y="2" width="5.4" height="5.4" rx="1.2" />)
    rects.push(<rect key="c" x="8.6" y="8.6" width="5.4" height="5.4" rx="1.2" />)
  } else {
    rects.push(<rect key="a" x="2" y="2" width="5.4" height="5.4" rx="1.2" />)
    rects.push(<rect key="b" x="8.6" y="2" width="5.4" height="5.4" rx="1.2" />)
    rects.push(<rect key="c" x="2" y="8.6" width="5.4" height="5.4" rx="1.2" />)
    rects.push(<rect key="d" x="8.6" y="8.6" width="5.4" height="5.4" rx="1.2" />)
  }
  return (
    <button
      type="button"
      title={title}
      onClick={onClick}
      className={`grid h-6 w-7 place-items-center rounded ${
        active ? 'bg-surface text-blue-500 shadow-sm' : 'text-muted-2 hover:text-fg'
      }`}
    >
      <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" fill="currentColor">
        {rects}
      </svg>
    </button>
  )
}

function SidebarItem({
  label,
  hint,
  active,
  icon,
  onClick,
  onContextMenu,
}: {
  label: string
  hint?: string
  active: boolean
  icon: React.ReactNode
  onClick: () => void
  onContextMenu?: (point: { clientX: number; clientY: number }) => void
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      onContextMenu={onContextMenu}
      className={`flex w-full items-center gap-2 rounded-md px-2 py-[5px] text-left text-[12px] ${
        active ? 'bg-sel font-medium text-fg-strong' : 'text-fg hover:bg-surface-3'
      }`}
    >
      {icon}
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {hint && <span className="shrink-0 text-[10px] text-muted-2">{hint}</span>}
    </button>
  )
}

type CaptionButtonProps = {
  label: string
  danger?: boolean
  onClick: () => void
  children: React.ReactNode
}

/** Window control that mimics the native caption buttons (46px, full header height). */
function CaptionButton({ label, danger, onClick, children }: CaptionButtonProps) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      onClick={onClick}
      className={`grid w-[46px] place-items-center text-muted transition-colors ${
        danger ? 'hover:bg-[#c42b1c] hover:text-white' : 'hover:bg-surface-3 hover:text-fg'
      }`}
    >
      <svg viewBox="0 0 16 16" className="h-4 w-4" fill="none" stroke="currentColor" strokeWidth="1.2">
        {children}
      </svg>
    </button>
  )
}

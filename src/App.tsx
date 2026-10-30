import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { MenuItem } from './ContextMenu'
import { OpsPanel } from './OpsPanel'
import { ModalContainer } from './ModalContainer'
import { useFileOperations } from './hooks/useFileOperations'
import { useDragAndDrop } from './hooks/useDragAndDrop'
import { useTreeAndGit } from './hooks/useTreeAndGit'
import { useWorkspaceLayout } from './hooks/useWorkspaceLayout'
import { useDirectoryScanner } from './hooks/useDirectoryScanner'
import { useAppSettings } from './hooks/useAppSettings'
import { usePreviewController } from './hooks/usePreviewController'
import { useQuickPlaces } from './hooks/useQuickPlaces'
import { useFileActionHandlers } from './hooks/useFileActionHandlers'
import type {
  BatchRenameModalState,
  DeleteConfirmModalState,
  PromptModalState,
} from './hooks/useFileActionHandlers'
import { usePaneContextMenus } from './hooks/usePaneContextMenus'
import { useGlobalShortcuts } from './hooks/useGlobalShortcuts'
import { useLaunchAndTray } from './hooks/useLaunchAndTray'
import { useWindowLifecycle } from './hooks/useWindowLifecycle'
import { TitleBar } from './components/TitleBar'
import { GUTTER, ResizeHandle } from './components/SplitView'
import { SidebarPanel } from './components/SidebarPanel'
import { PaneNodeRenderer } from './components/PaneNodeRenderer'
import { PreviewPanel } from './PreviewPanel'
import { SearchOverlay } from './SearchOverlay'
import { refreshIconCache } from './icons'
import {
  openPath,
  openWith,
  opCancel,
  quitApp,
  titlebarMenu,
} from './api'
import type { Lang, ListData, Place } from './types'
import { makeT } from './i18n'
import { DEFAULT_SETTINGS } from './persist'
import type { Settings } from './persist'
import { isDir, isFsPath, isInsideArchive, joinPath } from './paths'
import { formatBinding, resolveShortcuts } from './shortcuts'
const errorCode = (err: unknown): string =>
  err && typeof err === 'object' && 'code' in err && typeof err.code === 'string'
    ? err.code
    : String(err)

const pointOf = (ev: React.MouseEvent) => ({
  clientX: ev.clientX,
  clientY: ev.clientY,
  screenX: ev.screenX,
  screenY: ev.screenY,
  shiftKey: Boolean(ev.shiftKey),
})

const SIDEBAR_MIN = 140
const PREVIEW_MIN = 220

export default function App() {
  const [places, setPlaces] = useState<Place[]>([])
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [searchOpen, setSearchOpen] = useState(false)
  const [toast, setToast] = useState<string | null>(null)
  const [menu, setMenu] = useState<{ x: number; y: number; items: MenuItem[] } | null>(null)
  const [prompt, setPrompt] = useState<PromptModalState | null>(null)
  const [batchRename, setBatchRename] = useState<BatchRenameModalState | null>(null)
  const [deleteConfirm, setDeleteConfirm] = useState<DeleteConfirmModalState | null>(null)
  const [fileTools, setFileTools] = useState<
    { mode: 'hash'; paths: string[] } | { mode: 'dup'; root: string } | null
  >(null)
  const [compareOpen, setCompareOpen] = useState(false)
  const [historyOpen, setHistoryOpen] = useState(false)

  const layoutRef = useRef<HTMLDivElement>(null)
  const toastTimer = useRef<number | null>(null)
  const sidebarRef = useRef<HTMLElement | null>(null)

  const settingsRef = useRef<Settings>(DEFAULT_SETTINGS)
  const homeRef = useRef('C:\\')
  const readyRef = useRef(false)
  const listDataRef = useRef<Record<string, ListData>>({})
  const setListDataRef = useRef<React.Dispatch<React.SetStateAction<Record<string, ListData>>>>(() => undefined)
  const setPreviewOpenRef = useRef<React.Dispatch<React.SetStateAction<boolean>>>(() => undefined)
  const setSidebarWidthRef = useRef<React.Dispatch<React.SetStateAction<number>>>(() => undefined)
  const setPreviewWidthRef = useRef<React.Dispatch<React.SetStateAction<number>>>(() => undefined)
  const cancelDirRef = useRef<(tabId: string) => void>(() => undefined)

  const showToast = useCallback((text: string, ms = 2200) => {
    setToast(text)
    if (toastTimer.current) window.clearTimeout(toastTimer.current)
    toastTimer.current = window.setTimeout(() => setToast(null), ms)
  }, [])

  // Hook: workspace & layout
  const layout = useWorkspaceLayout({
    settingsRef,
    listDataRef,
    setListData: (action) => setListDataRef.current(action),
    showToast,
    t: makeT('zh-CN'),
    onCancelDir: (tabId) => cancelDirRef.current(tabId),
  })

  const {
    ws,
    setWs,
    wsRef,
    drag,
    paneEls,
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
    comparePaths,
    openFolders,
    openTabsForSearch,
    layoutInfo,
  } = layout

  // Hook: app settings
  const appSettings = useAppSettings({
    showToast,
    setWs,
    setPlaces,
    setDriveList: (v) => scanner.setDriveList(v),
    homeRef,
    readyRef,
    settingsRef,
    sidebarMin: SIDEBAR_MIN,
    previewMin: PREVIEW_MIN,
    setPreviewOpen: (v) => setPreviewOpenRef.current(v),
    setSidebarWidth: (v) => setSidebarWidthRef.current(v),
    setPreviewWidth: (v) => setPreviewWidthRef.current(v),
  })

  const {
    settings,
    updateSettings,
    flushSettings,
    setColWidths,
    setNameWidth,
    updateAvailable,
  } = appSettings

  const lang: Lang = settings.language
  const t = useMemo(() => makeT(lang), [lang])
  const keys = useMemo(() => resolveShortcuts(settings.shortcuts), [settings.shortcuts])

  // Hook: tree & git
  const {
    treeOpen,
    treeNodes,
    treeTruncated,
    gitByDir,
    gitByDirRef,
    invalidateTree,
    toggleTree,
    invalidateGit,
  } = useTreeAndGit({
    settingsRef,
    sidebarOpen: settings.sidebarOpen,
    showHidden: settings.showHidden,
    ws,
  })

  // Hook: directory scanner
  const scanner = useDirectoryScanner({
    wsRef,
    setWs,
    settingsRef,
    gitByDirRef,
    invalidateTree,
    invalidateGit,
    showToast,
  })

  const {
    listData,
    driveList,
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
    cancelDir,
  } = scanner

  setListDataRef.current = scanner.setListData
  listDataRef.current = listData
  cancelDirRef.current = (tabId) => void cancelDir(tabId).catch(() => undefined)

  // Keep tabs rows in memory & evict inactive
  useEffect(() => {
    evictInactiveTabs(ws)
  }, [ws, listData, evictInactiveTabs])

  // Hook: launch & tray
  useLaunchAndTray({
    ws,
    wsRef,
    setWs,
    newTab,
    navigate,
    startScan,
  })

  // Hook: file operations
  const {
    ops,
    setOps,
    elevating,
    runOp,
    retryElevated,
    extractArchive,
    runCompress,
    requestCompress,
    runSyncActions,
    undoLast,
    redoLast,
    undoTo,
    undoStack,
    redoStack,
    historyEpoch,
    passwordPrompt,
    setPasswordPrompt,
    extractTarget,
    setExtractTarget,
    compressTarget,
    setCompressTarget,
  } = useFileOperations({
    settingsRef,
    t,
    showToast,
    refreshPanes,
    invalidateTree,
    invalidateGit,
  })

  // Hook: drag and drop
  const { startRowDrag } = useDragAndDrop({
    wsRef,
    listData,
    t,
    showToast,
    refreshPanes,
    invalidateTree,
    invalidateGit,
    runOp,
  })

  // Hook: file action handlers
  const fileActions = useFileActionHandlers({
    wsRef,
    paneEls,
    listData,
    runOp,
    showToast,
    t,
    refreshPanes,
    invalidateTree,
    invalidateGit,
    setPrompt,
    setBatchRename,
    setDeleteConfirm,
    deleteConfirm,
  })

  const {
    pathsFor,
    focusList,
    deleteEntries,
    openProperties,
    startRename,
    startRenameMany,
    createFolder,
    pasteInto,
    copyPaths,
    copyCurrentPath,
    reportInvalidPath,
    copySelection,
  } = fileActions

  // Hook: quick places & sidebar
  const quickPlacesHook = useQuickPlaces({
    places,
    settings,
    updateSettings,
    sidebarMin: SIDEBAR_MIN,
    showToast,
    t,
    lang,
  })

  const {
    sidebarWidth,
    toggleSidebar,
    startSidebarResize,
    isPinnedPlace,
    togglePinPlace,
    quickPlaces,
    removePlace,
  } = quickPlacesHook

  const activePane = ws?.panes[ws.activePaneId]
  const activeTab = activePane ? ws?.tabs[activePane.activeTabId] : undefined
  const activeList = activeTab ? listData[activeTab.id] : undefined
  const selectedSet = activeList?.selected
  const selectedNames = useMemo(() => (selectedSet ? Array.from(selectedSet) : []), [selectedSet])
  const selectionKey = useMemo(() => selectedNames.join('\u0001'), [selectedNames])
  const previewName = selectedNames.length === 1 ? selectedNames[0] : ''

  // Hook: preview controller
  const previewCtrl = usePreviewController({
    settings,
    updateSettings,
    activeTab,
    previewName,
    selectionKey,
    previewMin: PREVIEW_MIN,
    showToast,
    t,
  })

  const {
    preview,
    previewOpen,
    setPreviewOpen,
    previewWidth,
    startPreviewResize,
    downloadPreview,
    openExternalPreview,
  } = previewCtrl

  setPreviewOpenRef.current = previewCtrl.setPreviewOpen
  setPreviewWidthRef.current = previewCtrl.setPreviewWidth
  setSidebarWidthRef.current = quickPlacesHook.setSidebarWidth

  const openTarget = useCallback(
    (targetPath: string) => {
      openPath(targetPath)
        .then(() => {
          if (isInsideArchive(targetPath)) refreshIconCache()
        })
        .catch((err: unknown) => {
          const code = errorCode(err)
          if (code === 'NO_ASSOCIATION') {
            void openWith(targetPath).catch((e: unknown) => {
              showToast(t('op.failed', { code: errorCode(e) }))
            })
            return
          }
          showToast(t('op.failed', { code }))
        })
    },
    [showToast, t],
  )

  // Hook: context menus
  const {
    openRowMenu,
    openBlankMenu,
    openTabMenu,
    openPlaceMenu,
    openTreeContextMenu,
    openDriveRowMenu,
    openDriveMenu,
  } = usePaneContextMenus({
    wsRef,
    listData,
    driveList,
    keys,
    t,
    lang,
    showToast,
    setMenu,
    navigate,
    openTarget,
    update,
    refreshPanes,
    manualRefresh,
    refreshDrives,
    toggleHidden,
    createFolder,
    pasteInto,
    copyPaths,
    startRename,
    startRenameMany,
    deleteEntries,
    extractArchive,
    setExtractTarget,
    requestCompress,
    setFileTools,
    setCompareOpen,
    closeTab,
    closeOthers,
    closeRight,
    togglePin,
    duplicateTab,
    openInNewPane,
    reopenTab,
    isPinnedPlace,
    togglePinPlace,
    removePlace,
  })

  // Hook: window lifecycle & native menu
  const { maximized, setMaximized, quitBusy, setQuitBusy } = useWindowLifecycle({
    ws,
    wsRef,
    readyRef,
    reportSaveError: appSettings.reportSaveError,
    flushSettings,
  })

  // Hook: global window keyboard shortcuts
  const overlayOpen = Boolean(
    settingsOpen ||
      searchOpen ||
      deleteConfirm ||
      prompt ||
      batchRename ||
      menu ||
      fileTools ||
      compareOpen ||
      quitBusy,
  )

  useGlobalShortcuts({
    keys,
    activePane,
    activeTab,
    activeList,
    previewName,
    overlayOpen,
    activateTab,
    newTab,
    closeTab,
    reopenTab,
    setSearchOpen,
    setSettingsOpen,
    setPreviewOpen,
    toggleSidebar,
    undoLast,
    redoLast,
    openExternalPreview,
  })
  const openInApp = useCallback(
    (path: string) => {
      setSettingsOpen(false)
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[current.activePaneId] ?? Object.values(current.panes)[0]
      const tab = pane ? current.tabs[pane.activeTabId] : undefined
      if (!tab) return
      navigate(tab.id, path)
    },
    [navigate, wsRef],
  )

  const openTreePath = useCallback(
    (path: string) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[current.activePaneId]
      const tab = pane ? current.tabs[pane.activeTabId] : undefined
      if (!tab) return
      if (!isFsPath(path)) {
        openTarget(path)
        return
      }
      navigate(tab.id, path)
    },
    [navigate, openTarget, wsRef],
  )

  // Mouse side buttons
  useEffect(() => {
    const onMouseUp = (ev: MouseEvent) => {
      if (ev.button !== 3 && ev.button !== 4) return
      ev.preventDefault()
      const current = wsRef.current
      if (!current) return
      const target = ev.target instanceof Element ? ev.target.closest<HTMLElement>('[data-pane-id]') : null
      if (!target && overlayOpen) return
      const pane = current.panes[target?.dataset.paneId ?? current.activePaneId]
      const tab = pane ? current.tabs[pane.activeTabId] : undefined
      if (tab) historyMove(tab.id, ev.button === 3 ? -1 : 1)
    }
    window.addEventListener('mouseup', onMouseUp)
    return () => window.removeEventListener('mouseup', onMouseUp)
  }, [historyMove, overlayOpen, wsRef])

  // tree reveal handled by SidebarPanel
  const selectedFolderPath = useMemo(() => {
    if (!activeTab?.path || selectedNames.length !== 1) return ''
    const entry = activeList?.entries.find((item) => item.name === selectedNames[0])
    if (!entry || !isDir(entry.attrs)) return ''
    return joinPath(activeTab.path, entry.name)
  }, [activeList?.entries, activeTab?.path, selectedNames])

  const treeFocus = selectedFolderPath || activeTab?.path || ''

  const treeSelected = useMemo(() => {
    if (selectedFolderPath) return selectedFolderPath
    return selectedNames.length ? '' : activeTab?.path ?? ''
  }, [activeTab?.path, selectedFolderPath, selectedNames.length])

  // tree scroll handled by SidebarPanel
  return (
    <div className="flex h-full flex-col bg-surface-2 text-fg">
      <TitleBar
        t={t}
        activePath={activeTab ? activeTab.path : ''}
        layoutCount={layoutInfo.count}
        layoutDir={layoutInfo.dir}
        applyLayout={applyLayout}
        toggleSidebar={toggleSidebar}
        sidebarOpen={settings.sidebarOpen}
        keys={keys}
        historyOpen={historyOpen}
        setHistoryOpen={setHistoryOpen}
        comparePathsLength={comparePaths.length}
        setCompareOpen={setCompareOpen}
        setSearchOpen={setSearchOpen}
        previewOpen={previewOpen}
        setPreviewOpen={setPreviewOpen}
        setSettingsOpen={setSettingsOpen}
        updateAvailable={updateAvailable}
        maximized={maximized}
        setMaximized={setMaximized}
        onContextMenu={(ev) => {
          ev.preventDefault()
          const dpr = window.devicePixelRatio || 1
          void titlebarMenu(Math.round(ev.screenX * dpr), Math.round(ev.screenY * dpr))
        }}
      />

      <div ref={layoutRef} className="flex min-h-0 flex-1">
        {settings.sidebarOpen ? (
          <>
            <SidebarPanel
              sidebarRef={sidebarRef}
              sidebarWidth={sidebarWidth}
              t={t}
              quickPlaces={quickPlaces}
              treeSelected={treeSelected}
              treeFocus={treeFocus}
              openTreePath={openTreePath}
              openPlaceMenu={openPlaceMenu}
              driveList={driveList}
              treeOpen={treeOpen}
              treeNodes={treeNodes}
              treeTruncated={treeTruncated}
              activePath={activeTab?.path ?? ''}
              toggleTree={toggleTree}
              openTreeContextMenu={openTreeContextMenu}
              pointOf={pointOf}
            />
            <ResizeHandle onPointerDown={startSidebarResize} />
          </>
        ) : null}

        <main className="flex min-h-0 min-w-0 flex-1 gap-0.5 p-0.5">
          {ws && (
            <PaneNodeRenderer
              node={ws.root}
              ws={ws}
              listData={listData}
              gitByDir={gitByDir}
              drag={drag}
              lang={lang}
              settings={settings}
              driveList={driveList}
              keys={keys}
              historyEpoch={historyEpoch}
              registerPane={registerPane}
              focusPane={focusPane}
              newTab={newTab}
              openTabAt={openTabAt}
              closeTab={closeTab}
              activateTab={activateTab}
              openTarget={openTarget}
              navigate={navigate}
              historyMove={historyMove}
              startTabDrag={startTabDrag}
              setPaneSort={setPaneSort}
              toggleHidden={toggleHidden}
              copySelection={copySelection}
              copyCurrentPath={copyCurrentPath}
              reportInvalidPath={reportInvalidPath}
              openRowMenu={openRowMenu}
              startRowDrag={startRowDrag}
              openBlankMenu={openBlankMenu}
              openTabMenu={openTabMenu}
              openDriveMenu={openDriveMenu}
              openDriveRowMenu={openDriveRowMenu}
              openProperties={openProperties}
              startRename={startRename}
              startRenameMany={startRenameMany}
              deleteEntries={deleteEntries}
              pasteInto={pasteInto}
              manualRefresh={manualRefresh}
              retryTab={retryTab}
              update={update}
              setColWidths={setColWidths}
              setNameWidth={setNameWidth}
              applyRatio={applyRatio}
            />
          )}
        </main>

        {previewOpen && (
          <>
            <ResizeHandle onPointerDown={startPreviewResize} />
            <PreviewPanel
              info={preview.info}
              loading={preview.loading}
              error={preview.error}
              name={previewName}
              path={activeTab?.path && previewName ? joinPath(activeTab.path, previewName) : ''}
              lang={lang}
              width={previewWidth}
              maxWidth={
                settings.sidebarOpen
                  ? `calc(50% - ${GUTTER * 2}px - ${sidebarWidth}px)`
                  : `calc(50% - ${GUTTER}px)`
              }
              externalKey={formatBinding(keys.externalPreview)}
              onClose={() => setPreviewOpen(false)}
              onOpenExternal={openExternalPreview}
              onDownload={downloadPreview}
            />
          </>
        )}
      </div>

      <OpsPanel
        ops={Object.values(ops)}
        lang={lang}
        elevating={elevating}
        onRetryElevated={retryElevated}
        onCancel={(opId) => void opCancel(opId)}
        onDismiss={(opId) => {
          setOps((prev) => {
            if (!(opId in prev)) return prev
            const next = { ...prev }
            delete next[opId]
            return next
          })
        }}
      />

      {toast && (
        <div className="pointer-events-none fixed bottom-4 left-1/2 z-50 -translate-x-1/2 rounded-md bg-fg px-3 py-1.5 text-[0.75rem] font-medium text-surface shadow-lg">
          {toast}
        </div>
      )}

      <ModalContainer
        lang={lang}
        t={t}
        settings={settings}
        updateSettings={updateSettings}
        activePane={activePane}
        focusList={focusList}
        showToast={showToast}
        runOp={runOp}
        extractArchive={extractArchive}
        runCompress={runCompress}
        runSyncActions={runSyncActions}
        openInApp={openInApp}
        pathsFor={pathsFor}
        openFolders={openFolders}
        comparePaths={comparePaths}
        historyOpen={historyOpen}
        setHistoryOpen={setHistoryOpen}
        historyEpoch={historyEpoch}
        undoStack={undoStack}
        redoStack={redoStack}
        undoTo={undoTo}
        redoLast={redoLast}
        menu={menu}
        setMenu={setMenu}
        passwordPrompt={passwordPrompt}
        setPasswordPrompt={setPasswordPrompt}
        extractTarget={extractTarget}
        setExtractTarget={setExtractTarget}
        prompt={prompt}
        setPrompt={setPrompt}
        compressTarget={compressTarget}
        setCompressTarget={setCompressTarget}
        batchRename={batchRename}
        setBatchRename={setBatchRename}
        fileTools={fileTools}
        setFileTools={setFileTools}
        compareOpen={compareOpen}
        setCompareOpen={setCompareOpen}
        deleteConfirm={deleteConfirm}
        setDeleteConfirm={setDeleteConfirm}
        quitBusy={quitBusy}
        setQuitBusy={setQuitBusy}
        quitApp={quitApp}
        settingsOpen={settingsOpen}
        setSettingsOpen={setSettingsOpen}
      />

      {searchOpen && activeTab && (
        <SearchOverlay
          root={activeTab.path || homeRef.current}
          openTabs={openTabsForSearch}
          lang={lang}
          fontSize={settings.fontSize}
          iconSize={settings.iconSize}
          preferEverything={settings.useEverything}
          showHidden={settings.showHidden}
          onClose={() => {
            setSearchOpen(false)
            if (activePane) focusList(activePane.id)
          }}
          onActivate={(item) => {
            if (item.isDir) navigate(activeTab.id, item.path)
            else openTarget(item.path)
          }}
          onOpen={(folder) => navigate(activeTab.id, folder)}
          onDelete={(paths) => runOp('delete', paths)}
        />
      )}
    </div>
  )
}

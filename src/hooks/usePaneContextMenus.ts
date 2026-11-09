import { useCallback, useEffect, useState } from 'react'
import type { MutableRefObject } from 'react'
import type { MenuItem } from '../ContextMenu'
import type { MenuPoint } from '../PaneView'
import type { Drive, Entry, Lang, ListData } from '../types'
import { samePath } from '../workspace'
import type { Workspace } from '../workspace'
import type { ShortcutId } from '../shortcuts'
import type { Translate } from '../i18n'
import { driveKindLabel } from '../i18n'
import { menuHint } from '../shortcuts'
import { entryPath, isFsPath, isInsideArchive, isWebdavPath, joinPath } from '../paths'
import {
  COMPRESS_FORMATS,
  driveEject,
  explorerOpen,
  shellMenuBackgroundShow,
  shellMenuShow,
  shellProperties,
} from '../api'
import type { CompressFormat } from '../api'
import { buildBlankMenu } from '../menus/blankMenu'
import { buildRowMenu } from '../menus/rowMenu'

export type QuickPlaceEntry = { key: string; label: string; path: string; custom: boolean }

const errorCode = (err: unknown): string =>
  err && typeof err === 'object' && 'code' in err && typeof err.code === 'string'
    ? err.code
    : String(err)

export interface UsePaneContextMenusOptions {
  wsRef: MutableRefObject<Workspace | null>
  listData: Record<string, ListData>
  driveList: Drive[]
  keys: Record<ShortcutId, string>
  t: Translate
  lang: Lang
  showToast: (msg: string) => void
  setMenu: (menu: { x: number; y: number; items: MenuItem[] } | null) => void
  navigate: (tabId: string, path: string) => void
  openTabAt?: (paneId: string, path: string) => void
  openTarget: (path: string) => void
  update: (tabId: string, fn: (d: ListData) => ListData) => void
  refreshPanes: () => void
  manualRefresh: () => void
  refreshDrives: () => void
  toggleHidden: (paneId: string) => void
  createFolder: (paneId: string) => void
  pasteInto: (paneId: string) => void
  copyPaths: (paths: string[]) => void
  startRename: (paneId: string, name: string, tabId?: string) => void
  startRenameMany: (paneId: string, names: string[]) => void
  deleteEntries: (paneId: string, names: string[], permanent: boolean) => void
  extractArchive: (archivePath: string, destDir: string) => void
  setExtractTarget: (val: { archive: string; initial: string } | null) => void
  requestCompress: (paths: string[], dir: string, format: CompressFormat) => void
  setFileTools: (val: { mode: 'hash'; paths: string[] } | { mode: 'dup'; root: string } | null) => void
  setCompareOpen: (open: boolean) => void
  startFolderSize: (tabId: string, path: string) => void
  closeTab: (paneId: string, tabId: string) => void
  closeOthers: (paneId: string, tabId: string) => void
  closeRight: (paneId: string, tabId: string) => void
  togglePin: (paneId: string, tabId: string) => void
  duplicateTab: (paneId: string, tabId: string) => void
  openInNewPane: (paneId: string, tabId: string) => void
  reopenTab: () => void
  isPinnedPlace: (path: string) => boolean
  togglePinPlace: (path: string) => void
  removePlace: (place: QuickPlaceEntry) => void
}

export function usePaneContextMenus({
  wsRef,
  listData,
  driveList,
  keys,
  t,
  showToast,
  setMenu,
  navigate,
  openTabAt,
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
  startFolderSize,
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
}: UsePaneContextMenusOptions) {
  /** A shell "New" command just ran; wait for the rescan that reveals the created item. */
  const [pendingNew, setPendingNew] = useState<{
    paneId: string
    tabId: string
    /** Folder the shell menu was opened in; navigating away invalidates the marker. */
    path: string
    before: Set<string>
    /** When the shell "New" command ran; only entries modified after it are candidates. */
    since: number
  } | null>(null)

  useEffect(() => {
    if (!pendingNew) return
    // A cancelled or failed shell "New" never adds an entry; without this the marker
    // would hijack the next unrelated new file that appears in the folder.
    const timer = window.setTimeout(() => setPendingNew(null), 5_000)
    return () => window.clearTimeout(timer)
  }, [pendingNew])

  useEffect(() => {
    if (!pendingNew) return
    const live = wsRef.current?.tabs[pendingNew.tabId]
    // The tab may be closed or sent elsewhere while the shell dialog is open; a rescan of
    // a different folder must not be read as the created item.
    if (!live || !samePath(live.path, pendingNew.path)) return
    const listing = listData[pendingNew.tabId]
    // Wait for the finished scan: a partial listing cannot tell a new entry from one the
    // enumeration has simply not reached yet.
    if (!listing || listing.loading) return
    const entries = listing.entries
    const created = entries.filter((entry) => !pendingNew.before.has(entry.name))
    if (!created.length) return
    // Another process (build, download, sync) can add a file in the same window. A truly
    // new item carries a modification time at (or just before, on coarse FAT clocks) the
    // menu action; renaming anything older is worse than skipping the convenience prompt.
    const fresh = created.filter((entry) => entry.mtime + 2_000 >= pendingNew.since)
    if (!fresh.length) return
    setPendingNew(null)
    if (fresh.length > 1) return
    const [entry] = fresh
    update(pendingNew.tabId, (data) => ({
      ...data,
      selected: new Set([entry.name]),
      anchor: entry.name,
    }))
    startRename(pendingNew.paneId, entry.name, pendingNew.tabId)
  }, [listData, pendingNew, startRename, update, wsRef])

  const rowFallback = useCallback(
    (paneId: string, entry: Entry, paths: string[]) =>
      buildRowMenu(
        {
          wsRef,
          t,
          keys,
          navigate,
          openTabAt,
          openTarget,
          showToast,
          copyPaths,
          startRename,
          startRenameMany,
          deleteEntries,
          pasteInto,
          extractArchive,
          setExtractTarget,
          requestCompress,
          setFileTools,
          isPinnedPlace,
          togglePinPlace,
        },
        paneId,
        entry,
        paths,
      ),
    [
      copyPaths,
      deleteEntries,
      extractArchive,
      isPinnedPlace,
      keys,
      navigate,
      openTabAt,
      openTarget,
      pasteInto,
      requestCompress,
      setExtractTarget,
      setFileTools,
      showToast,
      startRename,
      startRenameMany,
      t,
      togglePinPlace,
      wsRef,
    ],
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
      // Rows from a shell folder carry their own parsing names; joining the folder path
      // would produce a name the shell cannot resolve.
      const listed = listData[tab.id]?.entries
      const byName = listed ? new Map(listed.map((item) => [item.name, item])) : null
      const paths = names.map((name) => {
        const item = byName?.get(name)
        return item ? entryPath(tab.path, item) : joinPath(tab.path, name)
      })
      if (tab.path && (isInsideArchive(tab.path) || isWebdavPath(tab.path))) {
        setMenu({ x: point.clientX, y: point.clientY, items: rowFallback(paneId, entry, paths) })
        return
      }
      const dpr = window.devicePixelRatio || 1
      void shellMenuShow(
        paths,
        Math.round(point.screenX * dpr),
        Math.round(point.screenY * dpr),
        point.shiftKey,
        t('menu.hash'),
        { title: t('menu.extract'), here: t('menu.extractHere'), to: t('menu.extractTo') },
        {
          title: t('menu.compress'),
          formats: COMPRESS_FORMATS.map((format) => ({
            id: format,
            label: format.toUpperCase(),
          })),
        },
      )
        .then((outcome) => {
          if (outcome.action === 'hash') setFileTools({ mode: 'hash', paths })
          else if (outcome.action === 'extractHere') extractArchive(paths[0], tab.path)
          else if (outcome.action === 'extractTo') {
            setExtractTarget({ archive: paths[0], initial: tab.path })
          } else if (outcome.action === 'compress' && outcome.format) {
            requestCompress(paths, tab.path, outcome.format as CompressFormat)
          } else if (outcome.invoked) refreshPanes()
        })
        .catch(() => {
          setMenu({ x: point.clientX, y: point.clientY, items: rowFallback(paneId, entry, paths) })
        })
    },
    [
      extractArchive,
      listData,
      refreshPanes,
      requestCompress,
      rowFallback,
      setExtractTarget,
      setFileTools,
      setMenu,
      t,
      wsRef,
    ],
  )

  const blankFallbackMenu = useCallback(
    (paneId: string, point: MenuPoint) => {
      const items = buildBlankMenu(
        {
          wsRef,
          t,
          keys,
          update,
          manualRefresh,
          toggleHidden,
          createFolder,
          pasteInto,
          startFolderSize,
          setFileTools,
          setCompareOpen,
          showToast,
        },
        paneId,
        point,
      )
      if (items) setMenu({ x: point.clientX, y: point.clientY, items })
    },
    [
      createFolder,
      keys,
      manualRefresh,
      pasteInto,
      setCompareOpen,
      setFileTools,
      setMenu,
      showToast,
      startFolderSize,
      t,
      toggleHidden,
      update,
      wsRef,
    ],
  )

  const openBlankMenu = useCallback(
    (paneId: string, point: MenuPoint) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[paneId]
      const tab = pane ? current.tabs[pane.activeTabId] : undefined
      if (!pane || !tab) return
      if (!tab.path || isWebdavPath(tab.path) || isInsideArchive(tab.path)) {
        // Archive-internal paths look like filesystem paths but do not exist on disk; the
        // shell has no background menu for them, so go straight to the in-app fallback.
        blankFallbackMenu(paneId, point)
        return
      }
      // Captured once: the shell dialog is modal and this folder is what the marker means.
      const dir = tab.path
      const dpr = window.devicePixelRatio || 1
      const before = new Set((listData[tab.id]?.entries ?? []).map((entry) => entry.name))
      void shellMenuBackgroundShow(
        dir,
        Math.round(point.screenX * dpr),
        Math.round(point.screenY * dpr),
        point.shiftKey,
        pane.hidden ? t('list.hideHidden') : t('list.showHidden'),
        t('menu.findDuplicates'),
        t('compare.entry'),
      )
        .then((outcome) => {
          if (outcome.action === 'toggleHidden') toggleHidden(paneId)
          else if (outcome.action === 'findDuplicates') setFileTools({ mode: 'dup', root: dir })
          else if (outcome.action === 'compare') setCompareOpen(true)
          else if (outcome.action === 'newItem') {
            setPendingNew({ paneId, tabId: tab.id, path: dir, before, since: Date.now() })
            refreshPanes()
          } else if (outcome.invoked) refreshPanes()
        })
        .catch(() => blankFallbackMenu(paneId, point))
    },
    [blankFallbackMenu, listData, refreshPanes, setCompareOpen, setFileTools, startFolderSize, t, toggleHidden, wsRef],
  )

  const openTabMenu = useCallback(
    (paneId: string, tabId: string, point: MenuPoint) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[paneId]
      const tab = current.tabs[tabId]
      if (!pane || !tab) return
      const count = pane.tabIds.length
      const index = pane.tabIds.indexOf(tabId)
      const hasRight = index >= 0 && index < count - 1
      const isPinned = Boolean(tab.pinned)
      const otherUnpinned = pane.tabIds.some((id) => id !== tabId && !current.tabs[id]?.pinned)

      const items: MenuItem[] = [
        {
          kind: 'item',
          label: isPinned ? t('menu.unpinTab') : t('menu.pinTab'),
          onSelect: () => togglePin(paneId, tabId),
        },
        {
          kind: 'item',
          label: t('menu.duplicateTab'),
          onSelect: () => duplicateTab(paneId, tabId),
        },
        { kind: 'sep' },
        {
          kind: 'item',
          label: t('menu.closeTab'),
          hint: menuHint(keys.closeTab),
          onSelect: () => closeTab(paneId, tabId),
        },
        {
          kind: 'item',
          label: t('menu.closeOthers'),
          disabled: !otherUnpinned,
          onSelect: () => closeOthers(paneId, tabId),
        },
        {
          kind: 'item',
          label: t('menu.closeRight'),
          disabled: !hasRight,
          onSelect: () => closeRight(paneId, tabId),
        },
        {
          kind: 'item',
          label: t('menu.reopenTab'),
          hint: menuHint(keys.reopenTab),
          onSelect: reopenTab,
        },
        { kind: 'sep' },
        { kind: 'item', label: t('menu.copyPath'), disabled: !tab.path, onSelect: () => copyPaths([tab.path]) },
        { kind: 'item', label: t('menu.openInNewPane'), onSelect: () => openInNewPane(paneId, tabId) },
      ]
      setMenu({ x: point.clientX, y: point.clientY, items })
    },
    [
      closeOthers,
      closeRight,
      closeTab,
      copyPaths,
      duplicateTab,
      keys,
      openInNewPane,
      reopenTab,
      setMenu,
      t,
      togglePin,
      wsRef,
    ],
  )

  const openPlaceMenu = useCallback(
    (place: QuickPlaceEntry, point: MenuPoint) => {
      const current = wsRef.current
      const pane = current?.panes[current.activePaneId]
      const tab = pane && current ? current.tabs[pane.activeTabId] : undefined
      const items: MenuItem[] = [
        {
          kind: 'item',
          label: t('menu.open'),
          disabled: !tab,
          onSelect: () => {
            if (tab) navigate(tab.id, place.path)
          },
        },
        {
          kind: 'item',
          label: t('menu.openInNewTab'),
          disabled: !pane,
          onSelect: () => {
            if (!pane) return
            if (openTabAt) openTabAt(pane.id, place.path)
            else navigate(pane.activeTabId, place.path)
          },
        },
        { kind: 'sep' },
        { kind: 'item', label: t('menu.copyPath'), onSelect: () => copyPaths([place.path]) },
        { kind: 'sep' },
        { kind: 'item', label: t('sidebar.removePlace'), danger: true, onSelect: () => removePlace(place) },
      ]
      setMenu({ x: point.clientX, y: point.clientY, items })
    },
    [copyPaths, navigate, openTabAt, removePlace, setMenu, t, wsRef],
  )

  const openTreeContextMenu = useCallback(
    (targetPath: string, point: { clientX: number; clientY: number }) => {
      const current = wsRef.current
      const pane = current?.panes[current.activePaneId]
      const tab = pane && current ? current.tabs[pane.activeTabId] : undefined
      const remote = isWebdavPath(targetPath)
      const items: MenuItem[] = [
        {
          kind: 'item',
          label: t('menu.open'),
          disabled: !tab,
          onSelect: () => {
            if (tab) navigate(tab.id, targetPath)
          },
        },
        {
          kind: 'item',
          label: t('menu.openInNewTab'),
          disabled: !pane,
          onSelect: () => {
            if (!pane) return
            if (openTabAt) openTabAt(pane.id, targetPath)
            else navigate(pane.activeTabId, targetPath)
          },
        },
        ...(remote
          ? []
          : [
              {
                kind: 'item' as const,
                label: t('menu.openInExplorer'),
                onSelect: () =>
                  void explorerOpen(targetPath).catch((e: unknown) =>
                    showToast(t('op.failed', { code: errorCode(e) })),
                  ),
              },
            ]),
        {
          kind: 'item',
          label: isPinnedPlace(targetPath) ? t('sidebar.unpinPlace') : t('sidebar.pinPlace'),
          onSelect: () => togglePinPlace(targetPath),
        },
        { kind: 'sep' },
        {
          kind: 'item',
          label: t('menu.copyPath'),
          onSelect: () => copyPaths([targetPath]),
        },
        ...(remote
          ? []
          : [
              {
                kind: 'item' as const,
                label: t('menu.properties'),
                onSelect: () =>
                  void shellProperties(targetPath).catch((e: unknown) =>
                    showToast(t('op.failed', { code: errorCode(e) })),
                  ),
              },
            ]),
      ]
      setMenu({ x: point.clientX, y: point.clientY, items })
    },
    [copyPaths, isPinnedPlace, navigate, openTabAt, setMenu, showToast, t, togglePinPlace, wsRef],
  )

  const openDriveRowMenu = useCallback(
    (drive: Drive, point: MenuPoint) => {
      const current = wsRef.current
      const pane = current?.panes[current.activePaneId]
      const tab = pane && current ? current.tabs[pane.activeTabId] : undefined
      const items: MenuItem[] = [
        {
          kind: 'item',
          label: t('menu.open'),
          disabled: !tab,
          onSelect: () => {
            if (!tab) return
            if (isFsPath(drive.root) || isWebdavPath(drive.root)) navigate(tab.id, drive.root)
            else openTarget(drive.root)
          },
        },
        ...(isFsPath(drive.root)
          ? [
              {
                kind: 'item' as const,
                label: t('menu.openInNewTab'),
                disabled: !pane,
                onSelect: () => {
                  if (pane) {
                    if (openTabAt) openTabAt(pane.id, drive.root)
                    else navigate(pane.activeTabId, drive.root)
                  }
                },
              },
              {
                kind: 'item' as const,
                label: t('menu.openInExplorer'),
                onSelect: () =>
                  void explorerOpen(drive.root).catch((e: unknown) =>
                    showToast(t('op.failed', { code: errorCode(e) })),
                  ),
              },
            ]
          : []),
        ...(isWebdavPath(drive.root)
          ? []
          : [
              {
                kind: 'item' as const,
                label: t('menu.properties'),
                onSelect: () =>
                  void shellProperties(drive.root).catch((e: unknown) =>
                    showToast(t('op.failed', { code: errorCode(e) })),
                  ),
              },
            ]),
      ]
      if (drive.kind === 'removable' || drive.kind === 'cdrom') {
        items.push(
          { kind: 'sep' },
          {
            kind: 'item',
            label: t('menu.eject'),
            onSelect: () => {
              void driveEject(drive.root)
                .then(() => window.setTimeout(refreshDrives, 1200))
                .catch((e: unknown) => showToast(t('op.failed', { code: errorCode(e) })))
            },
          },
        )
      }
      setMenu({ x: point.clientX, y: point.clientY, items })
    },
    [navigate, openTabAt, openTarget, refreshDrives, setMenu, showToast, t, wsRef],
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
            : openTarget(drive.root),
      }))
      setMenu({ x: point.clientX, y: point.clientY, items })
    },
    [driveList, navigate, openTarget, setMenu, t, wsRef],
  )

  return {
    openRowMenu,
    rowFallback,
    blankFallbackMenu,
    openBlankMenu,
    openTabMenu,
    openPlaceMenu,
    openTreeContextMenu,
    openDriveRowMenu,
    openDriveMenu,
  }
}
